use crate::command;
use crate::fuse::LinaFs;
use fuser::Config;
use std::error::Error;
use std::path::Path;
use tokio::sync::oneshot;

pub async fn handle_mount(root: &str, args: &command::MountArgs) -> Result<(), Box<dyn Error>> {
    let root = root.to_string();
    let mp = args.mount_point.clone();

    let fs = LinaFs::new(&root, args.compressed)
        .await
        .map_err(|e| format!("Failed to initialize filesystem: {}", e))?;
    let bg = fuser::spawn_mount2(fs, Path::new(&mp), &Config::default())?;

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
