//! End-to-end tests against a real server process. Storage lives in a
//! `TempDir` (auto-cleaned) and bodies stay in memory, so the suite doesn't
//! litter the disk with big fixture files.

use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use reqwest::blocking::Client;
use reqwest::StatusCode;
use serial_test::serial;
use tempfile::TempDir;

/// Large-ish in-memory blob; small enough to keep the suite quick but big
/// enough to exercise the streamed, multi-worker IO path.
const BLOB_SIZE: usize = 8 * 1024 * 1024;

fn make_blob(seed: u8) -> Vec<u8> {
    let mut v = Vec::with_capacity(BLOB_SIZE);
    for i in 0..BLOB_SIZE {
        v.push(seed.wrapping_add((i / 4096) as u8));
    }
    v
}

/// Pick a free TCP port on loopback.
fn free_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    l.local_addr().expect("local addr").port()
}

fn wait_for_port(port: u16, timeout: Duration) {
    let start = Instant::now();
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    while start.elapsed() < timeout {
        if TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("server did not open port {} within {:?}", port, timeout);
}

/// Owns a freshly spawned server child and the scratch storage dir.
struct ServerGuard {
    child: Child,
    _dir: TempDir,
    root: PathBuf,
    http_port: u16,
    s3_port: u16,
}

impl ServerGuard {
    fn start() -> Self {
        Self::spawn(&[])
    }

    /// Spawn with extra env vars (e.g. to pin porter active/in-flight counts).
    fn start_with_env(extra: &[(&str, &str)]) -> Self {
        Self::spawn(extra)
    }

    fn spawn(extra: &[(&str, &str)]) -> Self {
        let dir = tempfile::Builder::new()
            .prefix("linastore-e2e-")
            .tempdir()
            .expect("tempdir");
        let root = dir.path().to_path_buf();

        let http_port = free_port();
        let advanced_port = free_port();
        let s3_port = free_port();

        let mut cmd = Command::new(env!("CARGO_BIN_EXE_linastore-server"));
        cmd.args(["start", "--foreground"])
            .current_dir(&root)
            .env("LINASTORE_HTTP_PORT", http_port.to_string())
            .env("LINASTORE_ADVANCED_PORT", advanced_port.to_string())
            .env("LINASTORE_S3_PORT", s3_port.to_string())
            .env("RUST_LOG", "info")
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        for (k, v) in extra {
            cmd.env(*k, *v);
        }
        let child = cmd.spawn().expect("spawn server");

        wait_for_port(http_port, Duration::from_secs(15));

        ServerGuard {
            child,
            _dir: dir,
            root,
            http_port,
            s3_port,
        }
    }

    fn s3(&self, bucket: &str, key: &str) -> String {
        format!("http://127.0.0.1:{}/{}/{}", self.s3_port, bucket, key)
    }

    fn http(&self, bucket: &str, key: &str) -> String {
        format!("http://127.0.0.1:{}/{}/{}", self.http_port, bucket, key)
    }

    /// Count object blob files under `linadata`, excluding DB files and logs.
    fn orphan_blob_count(&self) -> usize {
        count_blobs(&self.root.join("linadata"))
    }
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn count_blobs(dir: &Path) -> usize {
    let mut count = 0;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                // Skip the DB / logs dirs; source blobs live under <yyyy>/<mm>/.
                if name == "logs" {
                    continue;
                }
                count += count_blobs(&path);
            } else {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if name.ends_with(".db")
                    || name.ends_with(".db-wal")
                    || name.ends_with(".db-shm")
                {
                    continue;
                }
                count += 1;
            }
        }
    }
    count
}

#[test]
#[serial]
fn large_file_concurrency_and_no_leak() {
    let server = ServerGuard::start();
    let client = Client::builder()
        .timeout(Duration::from_secs(90))
        .build()
        .expect("client");

    let a = make_blob(0x5A);
    let b = make_blob(0x3C); // different content for overwrite

    // --- concurrent large PUTs (same content => exercises dedup) ---
    let mut handles = Vec::new();
    for i in 0..4 {
        let client = client.clone();
        let url = server.s3("e2e", &format!("blob{i}.bin"));
        let data = a.clone();
        handles.push(std::thread::spawn(move || {
            client.put(&url).body(data).send().unwrap()
        }));
    }
    for h in handles {
        assert_eq!(h.join().unwrap().status(), StatusCode::OK);
    }

    // --- concurrent large GETs, verify content ---
    let mut handles = Vec::new();
    for i in 0..4 {
        let client = client.clone();
        let url = server.s3("e2e", &format!("blob{i}.bin"));
        handles.push(std::thread::spawn(move || {
            let resp = client.get(&url).send().unwrap();
            (resp.status(), resp.bytes().unwrap().to_vec())
        }));
    }
    for h in handles {
        let (status, body) = h.join().unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, a, "GET content mismatch");
    }

    // --- same-key overwrite; must read the latest version ---
    let ow = server.s3("e2e", "blob0.bin");
    assert_eq!(
        client.put(&ow).body(b.clone()).send().unwrap().status(),
        StatusCode::OK
    );
    let got = client.get(&ow).send().unwrap();
    assert_eq!(got.status(), StatusCode::OK);
    assert_eq!(got.bytes().unwrap().to_vec(), b, "overwrite not visible");

    // --- concurrent deletes ---
    let mut handles = Vec::new();
    for i in 0..4 {
        let client = client.clone();
        let url = server.s3("e2e", &format!("blob{i}.bin"));
        handles.push(std::thread::spawn(move || client.delete(&url).send().unwrap()));
    }
    for h in handles {
        assert_eq!(h.join().unwrap().status(), StatusCode::NO_CONTENT);
    }

    // --- deleted key must now 404 ---
    let resp = client.get(&ow).send().unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // --- no orphan blobs may remain on disk ---
    assert_eq!(
        server.orphan_blob_count(),
        0,
        "orphan blob files leaked under {:?}",
        server.root
    );
}

/// A `Read` that emits `total` bytes slowly. Used to simulate a stalled,
/// long-running upload that should NOT occupy a processing slot.
struct PacingReader {
    remaining: usize,
    since_pace: usize,
}

impl Read for PacingReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.remaining == 0 {
            return Ok(0);
        }
        let n = buf.len().min(self.remaining);
        self.remaining -= n;
        self.since_pace += n;
        if self.since_pace >= 64 * 1024 {
            self.since_pace = 0;
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(n)
    }
}

#[test]
#[serial]
fn slow_uploads_do_not_block_processing() {
    // Pin active=2, in-flight=8. Use more uploads than active slots so that —
    // if streaming held an active slot — they'd starve every worker, but keep
    // the total (uploads + probe) under in-flight so the probe still starts.
    let server = ServerGuard::start_with_env(&[
        ("LINASTORE_PORTER_CONCURRENCY", "2"),
        ("LINASTORE_IN_FLIGHT", "8"),
    ]);
    let client = Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .expect("client");

    // Seed a small file we can read back.
    let seed = server.s3("e2e", "probe.txt");
    assert_eq!(client.put(&seed).body("probe".to_string()).send().unwrap().status(), StatusCode::OK);

    // 4 uploads > active slots (2); with the probe that's 5 < in-flight (8).
    const SLOW_UPLOADS: usize = 4;
    let handles: Vec<_> = (0..SLOW_UPLOADS)
        .map(|i| {
            let client = client.clone();
            let url = server.s3("e2e", &format!("slow{i}.bin"));
            std::thread::spawn(move || {
                let body = reqwest::blocking::Body::new(PacingReader {
                    remaining: 16 * 1024 * 1024,
                    since_pace: 0,
                });
                client.put(&url).body(body).send().unwrap().status()
            })
        })
        .collect();

    // Give the slow uploads a head start so they're mid-stream.
    std::thread::sleep(Duration::from_millis(500));

    // A small read must complete quickly even while uploads are in flight.
    let start = Instant::now();
    let resp = client.get(&seed).send().unwrap();
    let elapsed = start.elapsed();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.bytes().unwrap().to_vec(), b"probe");
    assert!(
        elapsed < Duration::from_secs(5),
        "small GET stalled {elapsed:?} behind slow uploads"
    );

    for h in handles {
        assert_eq!(h.join().unwrap(), StatusCode::OK);
    }
}

#[test]
#[serial]
fn basic_put_get_head_delete() {
    let server = ServerGuard::start();
    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("client");

    let url = server.s3("bkt", "hello.txt");
    let data = b"hello world".to_vec();

    assert_eq!(client.put(&url).body(data.clone()).send().unwrap().status(), StatusCode::OK);

    let get = client.get(&url).send().unwrap();
    assert_eq!(get.status(), StatusCode::OK);
    assert_eq!(get.bytes().unwrap().to_vec(), data);

    let head = client.head(&url).send().unwrap();
    assert_eq!(head.status(), StatusCode::OK);

    assert_eq!(client.delete(&url).send().unwrap().status(), StatusCode::NO_CONTENT);
    assert_eq!(client.get(&url).send().unwrap().status(), StatusCode::NOT_FOUND);

    assert_eq!(server.orphan_blob_count(), 0);
}

#[test]
#[serial]
fn http_get_reads_s3_object() {
    let server = ServerGuard::start();
    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("client");

    let key = "web/index.html";
    let s3_url = server.s3("web", key);
    let data = b"<html>hi</html>".to_vec();
    assert_eq!(
        client.put(&s3_url).body(data.clone()).send().unwrap().status(),
        StatusCode::OK
    );

    // The plain HTTP GET frontend serves the same object through the mapper.
    let resp = client.get(server.http("web", key)).send().unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.bytes().unwrap().to_vec(), data);
}
