use crate::error::{Context, Result, err_msg};
use nix::fcntl::{open, OFlag};
use nix::sys::stat::Mode;
use nix::unistd::{close, fork, setsid, ForkResult};
use std::os::fd::IntoRawFd;
use std::env;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tracing::{event, Level};
use tracing_subscriber::fmt::writer::MakeWriterExt;

use crate::shutdown::Shutdown;

/// Initialize logging with file output
fn init_logging(log_dir: &str) -> Result<()> {
    // Create log directory if it doesn't exist
    let log_path = Path::new(log_dir);
    if !log_path.exists() {
        fs::create_dir_all(log_path).context("Failed to create log directory")?;
    }

    // Open log file (append mode) to validate path early
    let log_file_path = log_path.join("linastore.log");
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_file_path)
        .context("Failed to open log file")?;

    let file_path = log_file_path.clone();
    let file_writer = move || {
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file_path)
            .expect("Failed to open log file")
    };

    let max_level = if cfg!(debug_assertions) {
        Level::DEBUG
    } else {
        Level::INFO
    };

    let builder = tracing_subscriber::fmt()
        .with_thread_ids(false)
        .with_file(false)
        .with_ansi(false)
        .with_target(false)
        .with_max_level(max_level);

    if cfg!(debug_assertions) {
        let writer = std::io::stdout.and(file_writer);
        builder.with_writer(writer).init();
    } else {
        builder.with_writer(file_writer).init();
    }

    Ok(())
}

/// Run the main server
fn daemonize() -> Result<()> {
    match unsafe { fork().context("Failed to fork process")? } {
        ForkResult::Parent { .. } => {
            std::process::exit(0);
        }
        ForkResult::Child => {
            setsid().context("Failed to create new session")?;

            // Redirect stdin/stdout/stderr to /dev/null
            let devnull = open("/dev/null", OFlag::O_RDWR, Mode::empty())
                .context("Failed to open /dev/null")?;
            let devnull_fd = devnull.into_raw_fd();
            unsafe {
                libc::dup2(devnull_fd, libc::STDIN_FILENO);
                libc::dup2(devnull_fd, libc::STDOUT_FILENO);
                libc::dup2(devnull_fd, libc::STDERR_FILENO);
            }
            close(devnull_fd).ok();
        }
    }

    Ok(())
}

pub async fn run_server(log_dir: Option<String>, daemon: bool) -> Result<()> {
    if daemon {
        daemonize()?;
    }

    // Write PID file after daemonizing so it contains the correct PID.
    write_pid_file()?;

    // Initialize logging
    let log_directory = log_dir.unwrap_or_else(|| {
        let current_dir = env::current_dir()
            .expect("Failed to get current directory")
            .to_str()
            .expect("Failed to convert path")
            .to_string();
        format!("{}/linadata/logs", current_dir)
    });

    init_logging(&log_directory)?;
    event!(
        tracing::Level::INFO,
        "Logging initialized, logs stored in: {}",
        log_directory
    );

    let current_dir = env::current_dir()
        .context("Failed to get current directory")?
        .to_str()
        .map(String::from)
        .context("Failed to convert current directory to string")?;

    // Initialize Shutdown Manager
    let shutdown_state = Shutdown::get_instance();

    // Initialize the order queue
    crate::conveyer::ConveyQueue::init();
    event!(tracing::Level::INFO, "Message queue initialized");

    // Initialize database
    let env_vars = crate::vars::EnvVar::get_instance();
    env_vars.validate()?;
    let db_conn = Arc::new(crate::db::get_db_connection(&env_vars.db_url).await?);
    event!(tracing::Level::INFO, "Database initialized");

    // Initialize auth manager with database connection
    crate::auth::init_auth_manager(Some(db_conn.clone()));
    event!(
        tracing::Level::INFO,
        "Auth manager initialized with database"
    );

    // Initialize admin user if password protection is enabled
    if let Err(e) = crate::auth::init_admin_user(&db_conn).await {
        event!(
            tracing::Level::ERROR,
            "Failed to initialize admin user: {:?}",
            e
        );
        return Err(e);
    }

    // Spawn session cleanup task
    let mut cleanup_handle = tokio::task::spawn(async move {
        crate::auth::cleanup_expired_sessions().await;
    });

    let mut porter_handle = tokio::task::spawn(async move {
        crate::porter::porter(&current_dir).await;
    });

    let mut front_handle = tokio::task::spawn(async move {
        crate::front::front().await;
    });

    // Graceful shutdown
    let shutdown_signal = tokio::signal::ctrl_c();
    tokio::pin!(shutdown_signal);

    match shutdown_signal.await {
        Ok(()) => {
            event!(tracing::Level::INFO, "Graceful shutdown");
            shutdown_state.shutdown();
        }
        Err(e) => event!(tracing::Level::ERROR, "Shutdown signal error: {:?}", e),
    }

    let shutdown_timeout = Duration::from_secs(5);

    if tokio::time::timeout(shutdown_timeout, &mut porter_handle)
        .await
        .is_err()
    {
        event!(
            tracing::Level::WARN,
            "Porter did not shut down in time, aborting"
        );
        porter_handle.abort();
    }

    if tokio::time::timeout(shutdown_timeout, &mut front_handle)
        .await
        .is_err()
    {
        event!(
            tracing::Level::WARN,
            "Front did not shut down in time, aborting"
        );
        front_handle.abort();
    }

    if tokio::time::timeout(shutdown_timeout, &mut cleanup_handle)
        .await
        .is_err()
    {
        event!(
            tracing::Level::WARN,
            "Session cleanup did not shut down in time, aborting"
        );
        cleanup_handle.abort();
    }

    Ok(())
}

/// Handle stop command
pub fn handle_stop(force: bool) -> Result<()> {
    // Initialize basic logging for stop command
    tracing_subscriber::fmt()
        .with_thread_ids(false)
        .with_file(false)
        .with_ansi(false)
        .with_target(false)
        .init();

    // Look for PID file
    let pid_file = env::current_dir()
        .context("Failed to get current directory")?
        .join("linastore")
        .join("linastore.pid");

    if !pid_file.exists() {
        eprintln!("No running LiNaStore server found (PID file not found)");
        return Ok(());
    }

    let pid_str = fs::read_to_string(&pid_file).context("Failed to read PID file")?;
    let pid: i32 = pid_str.trim().parse().context("Invalid PID in file")?;

    event!(
        tracing::Level::INFO,
        "Stopping LiNaStore server (PID: {})",
        pid
    );

    // Send SIGTERM for graceful shutdown
    let kill_result =
        unsafe { libc::kill(pid, if force { libc::SIGKILL } else { libc::SIGTERM }) };

    if kill_result != 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::ESRCH) {
            eprintln!("Server process not found (may have already stopped)");
            fs::remove_file(&pid_file).ok();
            return Ok(());
        }
        return Err(err_msg(format!("Failed to stop server: {}", err)));
    }

    // Remove PID file
    fs::remove_file(&pid_file).ok();

    if force {
        println!("LiNaStore server force stopped");
    } else {
        println!("LiNaStore server stopped gracefully");
    }

    Ok(())
}

/// Write PID file for process management
pub fn write_pid_file() -> Result<()> {
    let pid_dir = env::current_dir()
        .context("Failed to get current directory")?
        .join("linastore");

    if !pid_dir.exists() {
        fs::create_dir_all(&pid_dir).context("Failed to create linastore directory")?;
    }

    let pid_file = pid_dir.join("linastore.pid");
    let pid = std::process::id();
    fs::write(&pid_file, pid.to_string()).context("Failed to write PID file")?;

    Ok(())
}
