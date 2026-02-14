// LiNaStore End-to-End Tests
// This file contains comprehensive tests for the LiNaStore project

use std::fs;
use std::io::{self, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU16, Ordering};
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

// Test constants - matching server defaults from vars.rs
const BASE_HTTP_PORT: u16 = 8086;
const BASE_LINA_PORT: u16 = 8096;
const SERVER_STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
const SERVER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);

// Port counter for unique ports
static PORT_COUNTER: AtomicU16 = AtomicU16::new(0);

fn get_test_ports() -> (u16, u16) {
    let offset = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);
    (BASE_HTTP_PORT + offset, BASE_LINA_PORT + offset)
}

// Get the project directory
fn project_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

// Get the linastore binary path
fn linastore_binary() -> PathBuf {
    project_dir().join("target/release/linastore")
}

// Get the linastore-server binary path
fn linastore_server_binary() -> PathBuf {
    project_dir().join("target/release/linastore-server")
}

// Build binaries if they don't exist
fn ensure_binaries() {
    let linastore = linastore_binary();
    let server = linastore_server_binary();
    
    if !linastore.exists() || !server.exists() {
        let _ = Command::new("cargo")
            .args(["build", "--release"])
            .current_dir(project_dir())
            .status();
    }
}

// Test utilities
struct TestEnvironment {
    temp_dir: TempDir,
    storage_dir: PathBuf,
    test_files_dir: PathBuf,
    server_process: Option<std::process::Child>,
    http_port: u16,
    lina_port: u16,
}

impl TestEnvironment {
    fn new() -> io::Result<Self> {
        ensure_binaries();
        
        let temp_dir = TempDir::new()?;
        let storage_dir = temp_dir.path().join("storage");
        let test_files_dir = temp_dir.path().join("test_files");
        
        fs::create_dir_all(&storage_dir)?;
        fs::create_dir_all(&test_files_dir)?;
        
        Ok(TestEnvironment {
            temp_dir,
            storage_dir,
            test_files_dir,
            server_process: None,
            http_port: 0,
            lina_port: 0,
        })
    }
    
    fn create_test_file(&self, name: &str, content: &[u8]) -> io::Result<PathBuf> {
        let file_path = self.test_files_dir.join(name);
        let mut file = fs::File::create(&file_path)?;
        file.write_all(content)?;
        Ok(file_path)
    }
    
    fn create_test_file_with_size(&self, name: &str, size: usize) -> io::Result<PathBuf> {
        let file_path = self.test_files_dir.join(name);
        let mut file = fs::File::create(&file_path)?;
        let data = vec![0u8; size];
        file.write_all(&data)?;
        Ok(file_path)
    }
    
    fn start_server(&mut self) -> io::Result<()> {
        // Get unique ports for this test
        let (http_port, lina_port) = get_test_ports();
        self.http_port = http_port;
        self.lina_port = lina_port;
        
        // Find available ports
        let http_addr = format!("127.0.0.1:{}", http_port);
        let lina_addr = format!("127.0.0.1:{}", lina_port);
        
        // Check if ports are available
        if TcpListener::bind(&http_addr).is_err() {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                format!("HTTP port {} is already in use", http_port)
            ));
        }
        if TcpListener::bind(&lina_addr).is_err() {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                format!("LiNa port {} is already in use", lina_port)
            ));
        }
        
        // Start the server in the storage directory with custom ports
        let child = Command::new(linastore_server_binary())
            .env("LINASTORE_HTTP_PORT", http_port.to_string())
            .env("LINASTORE_ADVANCED_PORT", lina_port.to_string())
            .current_dir(&self.storage_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        
        self.server_process = Some(child);
        
        // Wait for server to start
        thread::sleep(SERVER_STARTUP_TIMEOUT);
        
        Ok(())
    }
    
    fn stop_server(&mut self) {
        if let Some(mut child) = self.server_process.take() {
            let _ = child.kill();
            let _ = wait_with_timeout(&mut child, SERVER_SHUTDOWN_TIMEOUT);
        }
    }
}

impl Drop for TestEnvironment {
    fn drop(&mut self) {
        self.stop_server();
    }
}

// Helper to wait for process with timeout
fn wait_with_timeout(child: &mut std::process::Child, timeout: Duration) -> io::Result<bool> {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        match child.try_wait() {
            Ok(Some(_)) => return Ok(true),
            Ok(None) => thread::sleep(Duration::from_millis(100)),
            Err(e) => return Err(e),
        }
    }
    Ok(false)
}

// CLI Operations Tests
#[test]
fn test_cli_put_single_file() {
    let env = TestEnvironment::new().expect("Failed to create test environment");
    let test_file = env.create_test_file("test.txt", b"Hello, World!")
        .expect("Failed to create test file");
    
    let output = Command::new(linastore_binary())
        .args([
            "storage", "put",
            test_file.to_str().unwrap()
        ])
        .current_dir(&env.storage_dir)
        .output()
        .expect("Failed to execute linastore put command");
    
    assert!(output.status.success(), "Put command failed: {:?}", output);
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("successfully"),
        "Expected success message in output"
    );
}

#[test]
fn test_cli_put_multiple_files() {
    let env = TestEnvironment::new().expect("Failed to create test environment");
    
    let file1 = env.create_test_file("file1.txt", b"Content 1")
        .expect("Failed to create test file 1");
    let file2 = env.create_test_file("file2.txt", b"Content 2")
        .expect("Failed to create test file 2");
    let file3 = env.create_test_file("file3.txt", b"Content 3")
        .expect("Failed to create test file 3");
    
    let output = Command::new(linastore_binary())
        .args([
            "storage", "put",
            file1.to_str().unwrap(),
            file2.to_str().unwrap(),
            file3.to_str().unwrap()
        ])
        .current_dir(&env.storage_dir)
        .output()
        .expect("Failed to execute linastore put command");
    
    assert!(output.status.success(), "Put command failed: {:?}", output);
}

#[test]
fn test_cli_get_file() {
    let env = TestEnvironment::new().expect("Failed to create test environment");
    let test_file = env.create_test_file("test.txt", b"Hello, World!")
        .expect("Failed to create test file");
    
    // Put the file
    let put_output = Command::new(linastore_binary())
        .args([
            "storage", "put",
            test_file.to_str().unwrap()
        ])
        .current_dir(&env.storage_dir)
        .output()
        .expect("Failed to execute linastore put command");
    assert!(put_output.status.success());
    
    // Get the file
    let get_dir = env.temp_dir.path().join("get_output");
    fs::create_dir_all(&get_dir).expect("Failed to create get output directory");
    
    let get_output = Command::new(linastore_binary())
        .args([
            "storage", "get",
            "test.txt",
            "--dest", get_dir.to_str().unwrap()
        ])
        .current_dir(&env.storage_dir)
        .output()
        .expect("Failed to execute linastore get command");
    
    assert!(get_output.status.success(), "Get command failed: {:?}", get_output);
    
    // Verify the retrieved file
    let retrieved_file = get_dir.join("test.txt");
    assert!(retrieved_file.exists(), "Retrieved file does not exist");
    
    let mut content = String::new();
    fs::File::open(&retrieved_file)
        .expect("Failed to open retrieved file")
        .read_to_string(&mut content)
        .expect("Failed to read retrieved file");
    
    assert_eq!(content, "Hello, World!", "Retrieved content does not match");
}

#[test]
fn test_cli_list_files() {
    let env = TestEnvironment::new().expect("Failed to create test environment");
    
    let file1 = env.create_test_file("file1.txt", b"Content 1")
        .expect("Failed to create test file 1");
    let file2 = env.create_test_file("file2.txt", b"Content 2")
        .expect("Failed to create test file 2");
    
    // Put files
    Command::new(linastore_binary())
        .args([
            "storage", "put",
            file1.to_str().unwrap(),
            file2.to_str().unwrap()
        ])
        .current_dir(&env.storage_dir)
        .output()
        .expect("Failed to execute linastore put command");
    
    // List files
    let list_output = Command::new(linastore_binary())
        .args([
            "storage", "list",
            "--num", "10"
        ])
        .current_dir(&env.storage_dir)
        .output()
        .expect("Failed to execute linastore list command");
    
    assert!(list_output.status.success(), "List command failed: {:?}", list_output);
    
    let stdout = String::from_utf8_lossy(&list_output.stdout);
    assert!(stdout.contains("file1.txt"), "Expected file1.txt in list output");
    assert!(stdout.contains("file2.txt"), "Expected file2.txt in list output");
}

#[test]
fn test_cli_delete_file() {
    let env = TestEnvironment::new().expect("Failed to create test environment");
    let test_file = env.create_test_file("test.txt", b"Hello, World!")
        .expect("Failed to create test file");
    
    // Put the file
    Command::new(linastore_binary())
        .args([
            "storage", "put",
            test_file.to_str().unwrap()
        ])
        .current_dir(&env.storage_dir)
        .output()
        .expect("Failed to execute linastore put command");
    
    // Delete the file
    let delete_output = Command::new(linastore_binary())
        .args([
            "storage", "delete",
            "test.txt"
        ])
        .current_dir(&env.storage_dir)
        .output()
        .expect("Failed to execute linastore delete command");
    
    assert!(delete_output.status.success(), "Delete command failed: {:?}", delete_output);
    
    // Verify file is deleted
    let list_output = Command::new(linastore_binary())
        .args([
            "storage", "list",
            "--num", "10"
        ])
        .current_dir(&env.storage_dir)
        .output()
        .expect("Failed to execute linastore list command");
    
    let stdout = String::from_utf8_lossy(&list_output.stdout);
    assert!(!stdout.contains("test.txt"), "File should be deleted but still exists in list");
}

#[test]
fn test_cli_compression() {
    let env = TestEnvironment::new().expect("Failed to create test environment");
    let test_file = env.create_test_file_with_size("large.txt", 1024 * 1024) // 1MB file
        .expect("Failed to create test file");
    
    // Put with compression
    let output = Command::new(linastore_binary())
        .args([
            "storage", "put",
            test_file.to_str().unwrap(),
            "-z"  // -z for compressed
        ])
        .current_dir(&env.storage_dir)
        .output()
        .expect("Failed to execute linastore put command");
    
    assert!(output.status.success(), "Put with compression failed: {:?}", output);
}

#[test]
fn test_cli_deduplication() {
    let env = TestEnvironment::new().expect("Failed to create test environment");
    
    let file1 = env.create_test_file("file1.txt", b"Same content")
        .expect("Failed to create test file 1");
    let file2 = env.create_test_file("file2.txt", b"Same content")
        .expect("Failed to create test file 2");
    
    // Put both files (same content, different names)
    Command::new(linastore_binary())
        .args([
            "storage", "put",
            file1.to_str().unwrap()
        ])
        .current_dir(&env.storage_dir)
        .output()
        .expect("Failed to execute first put command");
    
    let output = Command::new(linastore_binary())
        .args([
            "storage", "put",
            file2.to_str().unwrap()
        ])
        .current_dir(&env.storage_dir)
        .output()
        .expect("Failed to execute second put command");
    
    assert!(output.status.success(), "Second put with same content failed");
}

#[test]
fn test_cli_cover_flag() {
    let env = TestEnvironment::new().expect("Failed to create test environment");
    
    let file1 = env.create_test_file("test.txt", b"Original content")
        .expect("Failed to create test file 1");
    let file2 = env.create_test_file("test.txt", b"New content")
        .expect("Failed to create test file 2");
    
    // Put first file
    Command::new(linastore_binary())
        .args([
            "storage", "put",
            file1.to_str().unwrap()
        ])
        .current_dir(&env.storage_dir)
        .output()
        .expect("Failed to execute first put command");
    
    // Put second file with cover flag
    let output = Command::new(linastore_binary())
        .args([
            "storage", "put",
            file2.to_str().unwrap(),
            "-c"  // -c for cover
        ])
        .current_dir(&env.storage_dir)
        .output()
        .expect("Failed to execute second put command");
    
    assert!(output.status.success(), "Put with cover flag failed");
    
    // Verify the content was updated
    let get_dir = env.temp_dir.path().join("get_output");
    fs::create_dir_all(&get_dir).expect("Failed to create get output directory");
    
    Command::new(linastore_binary())
        .args([
            "storage", "get",
            "test.txt",
            "--dest", get_dir.to_str().unwrap()
        ])
        .current_dir(&env.storage_dir)
        .output()
        .expect("Failed to execute get command");
    
    let retrieved_file = get_dir.join("test.txt");
    let mut content = String::new();
    fs::File::open(&retrieved_file)
        .expect("Failed to open retrieved file")
        .read_to_string(&mut content)
        .expect("Failed to read retrieved file");
    
    assert_eq!(content, "New content", "Content should be updated with cover flag");
}

#[test]
fn test_cli_list_by_extension() {
    let env = TestEnvironment::new().expect("Failed to create test environment");
    
    let file1 = env.create_test_file("file1.txt", b"Content 1")
        .expect("Failed to create test file 1");
    let file2 = env.create_test_file("file2.json", b"Content 2")
        .expect("Failed to create test file 2");
    let file3 = env.create_test_file("file3.txt", b"Content 3")
        .expect("Failed to create test file 3");
    
    // Put files
    Command::new(linastore_binary())
        .args([
            "storage", "put",
            file1.to_str().unwrap(),
            file2.to_str().unwrap(),
            file3.to_str().unwrap()
        ])
        .current_dir(&env.storage_dir)
        .output()
        .expect("Failed to execute linastore put command");
    
    // List only .txt files
    let list_output = Command::new(linastore_binary())
        .args([
            "storage", "list",
            "--ext", "txt",
            "--num", "10"
        ])
        .current_dir(&env.storage_dir)
        .output()
        .expect("Failed to execute linastore list command");
    
    assert!(list_output.status.success(), "List by extension failed: {:?}", list_output);
    
    let stdout = String::from_utf8_lossy(&list_output.stdout);
    assert!(stdout.contains("file1.txt"), "Expected file1.txt in list output");
    assert!(stdout.contains("file3.txt"), "Expected file3.txt in list output");
    assert!(!stdout.contains("file2.json"), "Should not contain .json files");
}

// HTTP Server Tests
#[test]
fn test_http_get_file() {
    let mut env = TestEnvironment::new().expect("Failed to create test environment");
    let test_file = env.create_test_file("test.txt", b"Hello, World!")
        .expect("Failed to create test file");
    
    // Put the file first
    Command::new(linastore_binary())
        .args([
            "storage", "put",
            test_file.to_str().unwrap()
        ])
        .current_dir(&env.storage_dir)
        .output()
        .expect("Failed to execute linastore put command");
    
    // Start server
    env.start_server().expect("Failed to start server");
    
    // Wait a bit more for server to be ready
    thread::sleep(Duration::from_secs(2));
    
    // Make HTTP GET request
    let response = reqwest::blocking::get(&format!("http://127.0.0.1:{}/test.txt", env.http_port))
        .expect("Failed to make HTTP request");
    
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    
    let content = response.text().expect("Failed to read response body");
    assert_eq!(content, "Hello, World!");
}

#[test]
fn test_http_get_nonexistent_file() {
    let mut env = TestEnvironment::new().expect("Failed to create test environment");
    
    // Start server
    env.start_server().expect("Failed to start server");
    
    thread::sleep(Duration::from_secs(2));
    
    // Try to get non-existent file
    let response = reqwest::blocking::get(&format!("http://127.0.0.1:{}/nonexistent.txt", env.http_port))
        .expect("Failed to make HTTP request");
    
    // Server returns 200 even for non-existent files (with empty body)
    // This is the actual server behavior
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    
    let content = response.text().expect("Failed to read response body");
    assert!(content.is_empty(), "Non-existent file should return empty content");
}

#[test]
fn test_http_mime_types() {
    let mut env = TestEnvironment::new().expect("Failed to create test environment");
    
    let json_file = env.create_test_file("test.json", b"{}")
        .expect("Failed to create test json file");
    
    // Put the file
    Command::new(linastore_binary())
        .args([
            "storage", "put",
            json_file.to_str().unwrap()
        ])
        .current_dir(&env.storage_dir)
        .output()
        .expect("Failed to execute linastore put command");
    
    // Start server
    env.start_server().expect("Failed to start server");
    
    thread::sleep(Duration::from_secs(2));
    
    // Make HTTP GET request
    let response = reqwest::blocking::get(&format!("http://127.0.0.1:{}/test.json", env.http_port))
        .expect("Failed to make HTTP request");
    
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    
    let content_type = response.headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .expect("Content-Type header not found");
    
    assert_eq!(content_type, "application/json");
}

// LiNa Protocol Tests
#[test]
fn test_lina_protocol_read() {
    let mut env = TestEnvironment::new().expect("Failed to create test environment");
    let test_file = env.create_test_file("test.txt", b"Hello, World!")
        .expect("Failed to create test file");
    
    // Put the file first
    Command::new(linastore_binary())
        .args([
            "storage", "put",
            test_file.to_str().unwrap()
        ])
        .current_dir(&env.storage_dir)
        .output()
        .expect("Failed to execute linastore put command");
    
    // Start server
    env.start_server().expect("Failed to start server");
    
    thread::sleep(Duration::from_secs(2));
    
    // Create LiNa protocol read request
    let mut stream = std::net::TcpStream::connect(format!("127.0.0.1:{}", env.lina_port))
        .expect("Failed to connect to LiNa server");
    stream.set_read_timeout(Some(Duration::from_secs(5))).expect("Failed to set read timeout");
    
    let flags: u8 = 0x40; // Read flag
    let mut identifier = [0u8; 255];
    let name = b"test.txt";
    identifier[..name.len()].copy_from_slice(name);
    let length: u32 = 0;
    
    // Calculate checksum
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&identifier);
    hasher.update(&length.to_le_bytes());
    let checksum = hasher.finalize();
    
    let mut request = Vec::new();
    request.push(flags);
    request.extend_from_slice(&identifier);
    request.extend_from_slice(&length.to_le_bytes());
    request.extend_from_slice(&checksum.to_le_bytes());
    
    stream.write_all(&request).expect("Failed to send request");
    
    // Read response header (status + identifier + length + checksum = 264 bytes)
    let mut header = [0u8; 264];
    stream.read_exact(&mut header).expect("Failed to read response header");
    
    // Parse length to determine how much data to read
    let data_len = u32::from_le_bytes([header[256], header[257], header[258], header[259]]) as usize;
    
    // Read the data if present
    let mut data = vec![0u8; data_len];
    if data_len > 0 {
        stream.read_exact(&mut data).expect("Failed to read response data");
    }
    
    // Combine header and data
    let mut response = header.to_vec();
    response.extend_from_slice(&data);
    
    // Verify response
    assert!(!response.is_empty(), "Response should not be empty");
    
    // Verify we got the file data
    assert!(response.len() >= 264, "Response should be at least 264 bytes");
    assert!(data.contains(&b'H'), "Response should contain file data 'Hello, World!'");
}

#[test]
fn test_lina_protocol_write() {
    let mut env = TestEnvironment::new().expect("Failed to create test environment");
    
    // Start server
    env.start_server().expect("Failed to start server");
    
    thread::sleep(Duration::from_secs(2));
    
    // Create LiNa protocol write request
    let mut stream = std::net::TcpStream::connect(format!("127.0.0.1:{}", env.lina_port))
        .expect("Failed to connect to LiNa server");
    
    let flags: u8 = 0x80; // Write flag
    let mut identifier = [0u8; 255];
    let name = b"new.txt";
    identifier[..name.len()].copy_from_slice(name);
    let data = b"New file content";
    let length: u32 = data.len() as u32;
    
    // Calculate checksum
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&identifier);
    hasher.update(&length.to_le_bytes());
    hasher.update(data);
    let checksum = hasher.finalize();
    
    let mut request = Vec::new();
    request.push(flags);
    request.extend_from_slice(&identifier);
    request.extend_from_slice(&length.to_le_bytes());
    request.extend_from_slice(&checksum.to_le_bytes());
    request.extend_from_slice(data);
    
    stream.write_all(&request).expect("Failed to send request");
    
    // Read response
    let mut response = Vec::new();
    stream.read_to_end(&mut response).expect("Failed to read response");
    
    assert!(!response.is_empty(), "Response should not be empty");
    
    // Verify file was written
    thread::sleep(Duration::from_secs(1));
    
    let list_output = Command::new(linastore_binary())
        .args([
            "storage", "list",
            "--num", "10"
        ])
        .current_dir(&env.storage_dir)
        .output()
        .expect("Failed to execute linastore list command");
    
    let stdout = String::from_utf8_lossy(&list_output.stdout);
    assert!(stdout.contains("new.txt"), "File should be created via LiNa protocol");
}

#[test]
fn test_lina_protocol_delete() {
    let mut env = TestEnvironment::new().expect("Failed to create test environment");
    let test_file = env.create_test_file("test.txt", b"Hello, World!")
        .expect("Failed to create test file");
    
    // Put the file first
    Command::new(linastore_binary())
        .args([
            "storage", "put",
            test_file.to_str().unwrap()
        ])
        .current_dir(&env.storage_dir)
        .output()
        .expect("Failed to execute linastore put command");
    
    // Start server
    env.start_server().expect("Failed to start server");
    
    thread::sleep(Duration::from_secs(2));
    
    // Create LiNa protocol delete request
    let mut stream = std::net::TcpStream::connect(format!("127.0.0.1:{}", env.lina_port))
        .expect("Failed to connect to LiNa server");
    stream.set_read_timeout(Some(Duration::from_secs(5))).expect("Failed to set read timeout");
    
    let flags: u8 = 0xC0; // Delete flag
    let mut identifier = [0u8; 255];
    let name = b"test.txt";
    identifier[..name.len()].copy_from_slice(name);
    let length: u32 = 0;
    
    // Calculate checksum
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&identifier);
    hasher.update(&length.to_le_bytes());
    let checksum = hasher.finalize();
    
    let mut request = Vec::new();
    request.push(flags);
    request.extend_from_slice(&identifier);
    request.extend_from_slice(&length.to_le_bytes());
    request.extend_from_slice(&checksum.to_le_bytes());
    
    stream.write_all(&request).expect("Failed to send request");
    
    // Read response header (status + identifier + length + checksum = 264 bytes)
    let mut header = [0u8; 264];
    stream.read_exact(&mut header).expect("Failed to read response header");
    
    // Parse length to determine how much data to read
    let data_len = u32::from_le_bytes([header[256], header[257], header[258], header[259]]) as usize;
    
    // Read the data if present
    let mut data = vec![0u8; data_len];
    if data_len > 0 {
        stream.read_exact(&mut data).expect("Failed to read response data");
    }
    
    // Combine header and data
    let mut response = header.to_vec();
    response.extend_from_slice(&data);
    
    // Verify we got a response
    assert!(!response.is_empty(), "Response should not be empty");
    assert!(response.len() >= 264, "Response should be at least 264 bytes");
    
    // Verify file was deleted
    thread::sleep(Duration::from_secs(1));
    
    let list_output = Command::new(linastore_binary())
        .args([
            "storage", "list",
            "--num", "10"
        ])
        .current_dir(&env.storage_dir)
        .output()
        .expect("Failed to execute linastore list command");
    
    let stdout = String::from_utf8_lossy(&list_output.stdout);
    assert!(!stdout.contains("test.txt"), "File should be deleted via LiNa protocol");
}

// Edge Cases and Error Handling Tests
#[test]
fn test_nonexistent_file_get() {
    let env = TestEnvironment::new().expect("Failed to create test environment");
    
    let get_dir = env.temp_dir.path().join("get_output");
    fs::create_dir_all(&get_dir).expect("Failed to create get output directory");
    
    let output = Command::new(linastore_binary())
        .args([
            "storage", "get",
            "nonexistent.txt",
            "--dest", get_dir.to_str().unwrap()
        ])
        .current_dir(&env.storage_dir)
        .output()
        .expect("Failed to execute linastore get command");
    
    assert!(!output.status.success(), "Get nonexistent file should fail");
}

#[test]
fn test_empty_storage_list() {
    let env = TestEnvironment::new().expect("Failed to create test environment");
    
    let output = Command::new(linastore_binary())
        .args([
            "storage", "list",
            "--num", "10"
        ])
        .current_dir(&env.storage_dir)
        .output()
        .expect("Failed to execute linastore list command");
    
    assert!(output.status.success(), "List empty storage should succeed");
    
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("No files found") || stdout.is_empty(), 
        "Expected 'No files found' message or empty output");
}

#[test]
fn test_large_file_handling() {
    let env = TestEnvironment::new().expect("Failed to create test environment");
    let test_file = env.create_test_file_with_size("large.bin", 10 * 1024 * 1024) // 10MB file
        .expect("Failed to create test file");
    
    let output = Command::new(linastore_binary())
        .args([
            "storage", "put",
            test_file.to_str().unwrap()
        ])
        .current_dir(&env.storage_dir)
        .output()
        .expect("Failed to execute linastore put command");
    
    assert!(output.status.success(), "Put large file failed");
}

#[test]
fn test_special_characters_filename() {
    let env = TestEnvironment::new().expect("Failed to create test environment");
    
    // Note: Some special characters might not be supported by the filesystem
    let test_file = env.create_test_file("test-file_123.txt", b"Content")
        .expect("Failed to create test file");
    
    let output = Command::new(linastore_binary())
        .args([
            "storage", "put",
            test_file.to_str().unwrap()
        ])
        .current_dir(&env.storage_dir)
        .output()
        .expect("Failed to execute linastore put command");
    
    assert!(output.status.success(), "Put file with special characters failed");
}

// Integration Tests - Multiple Operations
#[test]
fn test_full_workflow() {
    let env = TestEnvironment::new().expect("Failed to create test environment");
    
    // Step 1: Create and put files
    let file1 = env.create_test_file("doc1.txt", b"Document 1")
        .expect("Failed to create test file 1");
    let file2 = env.create_test_file("doc2.txt", b"Document 2")
        .expect("Failed to create test file 2");
    
    Command::new(linastore_binary())
        .args([
            "storage", "put",
            file1.to_str().unwrap(),
            file2.to_str().unwrap()
        ])
        .current_dir(&env.storage_dir)
        .output()
        .expect("Failed to execute linastore put command");
    
    // Step 2: List files
    let list_output = Command::new(linastore_binary())
        .args([
            "storage", "list",
            "--num", "10"
        ])
        .current_dir(&env.storage_dir)
        .output()
        .expect("Failed to execute linastore list command");
    
    assert!(list_output.status.success());
    let stdout = String::from_utf8_lossy(&list_output.stdout);
    assert!(stdout.contains("doc1.txt"));
    assert!(stdout.contains("doc2.txt"));
    
    // Step 3: Get files
    let get_dir = env.temp_dir.path().join("get_output");
    fs::create_dir_all(&get_dir).expect("Failed to create get output directory");
    
    Command::new(linastore_binary())
        .args([
            "storage", "get",
            "doc1.txt",
            "doc2.txt",
            "--dest", get_dir.to_str().unwrap()
        ])
        .current_dir(&env.storage_dir)
        .output()
        .expect("Failed to execute linastore get command");
    
    // Step 4: Verify retrieved files
    let retrieved1 = get_dir.join("doc1.txt");
    let retrieved2 = get_dir.join("doc2.txt");
    
    assert!(retrieved1.exists());
    assert!(retrieved2.exists());
    
    let mut content1 = String::new();
    fs::File::open(&retrieved1)
        .expect("Failed to open retrieved file 1")
        .read_to_string(&mut content1)
        .expect("Failed to read retrieved file 1");
    
    let mut content2 = String::new();
    fs::File::open(&retrieved2)
        .expect("Failed to open retrieved file 2")
        .read_to_string(&mut content2)
        .expect("Failed to read retrieved file 2");
    
    assert_eq!(content1, "Document 1");
    assert_eq!(content2, "Document 2");
    
    // Step 5: Delete one file
    Command::new(linastore_binary())
        .args([
            "storage", "delete",
            "doc1.txt"
        ])
        .current_dir(&env.storage_dir)
        .output()
        .expect("Failed to execute linastore delete command");
    
    // Step 6: Verify deletion
    let list_output2 = Command::new(linastore_binary())
        .args([
            "storage", "list",
            "--num", "10"
        ])
        .current_dir(&env.storage_dir)
        .output()
        .expect("Failed to execute linastore list command");
    
    let stdout2 = String::from_utf8_lossy(&list_output2.stdout);
    assert!(!stdout2.contains("doc1.txt"), "doc1.txt should be deleted");
    assert!(stdout2.contains("doc2.txt"), "doc2.txt should still exist");
}

// Performance Tests
#[test]
#[ignore] // Run manually for performance testing
fn test_bulk_operations() {
    let env = TestEnvironment::new().expect("Failed to create test environment");
    
    // Create 100 files
    let mut files = Vec::new();
    for i in 0..100 {
        let file = env.create_test_file(&format!("file{}.txt", i), b"Test content")
            .expect("Failed to create test file");
        files.push(file);
    }
    
    let start = std::time::Instant::now();
    
    // Put all files
    for file in &files {
        Command::new(linastore_binary())
            .args([
                "storage", "put",
                file.to_str().unwrap()
            ])
            .current_dir(&env.storage_dir)
            .output()
            .expect("Failed to execute linastore put command");
    }
    
    let put_duration = start.elapsed();
    println!("Time to put 100 files: {:?}", put_duration);
    
    // List files
    let start = std::time::Instant::now();
    Command::new(linastore_binary())
        .args([
            "storage", "list",
            "--num", "100"
        ])
        .current_dir(&env.storage_dir)
        .output()
        .expect("Failed to execute linastore list command");
    
    let list_duration = start.elapsed();
    println!("Time to list 100 files: {:?}", list_duration);
}
