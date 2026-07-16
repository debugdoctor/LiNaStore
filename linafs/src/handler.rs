use crate::command;
use crate::fuse::LinaFs;
use fuser::{Config, MountOption};
use std::error::Error;
use std::path::Path;
#[cfg(target_os = "macos")]
use std::process::Command;
#[cfg(target_os = "macos")]
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

pub async fn handle_mount(root: &str, args: &command::MountArgs) -> Result<(), Box<dyn Error>> {
    let root = root.to_string();
    let mp = args.mount_point.clone();

    ensure_fuse_available()?;

    let fs = LinaFs::new(&root, args.compressed)
        .await
        .map_err(|e| format!("Failed to initialize filesystem: {}", e))?;
    let mut config = Config::default();
    config.mount_options = vec![
        MountOption::FSName("linafs".to_string()),
        MountOption::Subtype("linafs".to_string()),
        MountOption::CUSTOM("volname=linafs".to_string()),
    ];

    let bg = fuser::spawn_mount2(fs, Path::new(&mp), &config)
        .map_err(|e| format!("Failed to mount at {}: {}", mp, e))?;

    if args.foreground {
        println!("Mounted at {}. Press Ctrl+C to unmount.", mp);
        tokio::signal::ctrl_c().await?;
        println!("Unmounting {}...", mp);
        drop(bg);
    } else {
        println!(
            "Mounted at {} (PID: {}). Use 'linafs umount {}' to unmount.",
            mp,
            std::process::id(),
            mp
        );
        let (tx, rx) = oneshot::channel();
        std::thread::spawn(move || {
            let _ = bg.join();
            let _ = tx.send(());
        });
        let _ = rx.await;
        println!("Unmounted {}", mp);
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn ensure_fuse_available() -> Result<(), Box<dyn Error>> {
    if macos_fuse_device_exists() {
        return Ok(());
    }

    let loader = "/Library/Filesystems/macfuse.fs/Contents/Resources/load_macfuse";
    let status = run_loader_with_timeout(loader, Duration::from_secs(5))?;

    if status.success() && macos_fuse_device_exists() {
        return Ok(());
    }

    Err(format!(
        "macFUSE is not loaded. Install/approve macFUSE in System Settings, then try again. {} exited with {}.",
        loader, status
    )
    .into())
}

#[cfg(target_os = "macos")]
fn macos_fuse_device_exists() -> bool {
    let Ok(entries) = std::fs::read_dir("/dev") else {
        return false;
    };

    entries.filter_map(Result::ok).any(|entry| {
        entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.contains("fuse"))
    })
}

#[cfg(target_os = "macos")]
fn run_loader_with_timeout(
    loader: &str,
    timeout: Duration,
) -> Result<std::process::ExitStatus, Box<dyn Error>> {
    let mut child = Command::new(loader)
        .spawn()
        .map_err(|e| format!("Failed to run macFUSE loader at {}: {}", loader, e))?;
    let started = Instant::now();

    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }

        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "Timed out waiting for macFUSE loader at {}. Approve macFUSE in System Settings and try again.",
                loader
            )
            .into());
        }

        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(not(target_os = "macos"))]
fn ensure_fuse_available() -> Result<(), Box<dyn Error>> {
    Ok(())
}

pub async fn handle_umount(args: &command::UmountArgs) -> Result<(), Box<dyn Error>> {
    let mp = &args.mount_point;

    #[cfg(target_os = "linux")]
    let status = std::process::Command::new("fusermount")
        .args(["-u", mp])
        .status()?;

    #[cfg(not(target_os = "linux"))]
    let status = std::process::Command::new("umount")
        .arg(mp)
        .status()?;

    if status.success() {
        println!("Unmounted {}", mp);
    } else {
        return Err(format!("Failed to unmount {}", mp).into());
    }
    Ok(())
}
