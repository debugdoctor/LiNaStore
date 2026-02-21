mod auth;
mod conveyer;
mod db;
mod dtos;
mod error;
mod front;
mod porter;
mod shutdown;
mod utils;
mod vars;

use clap::{Parser, Subcommand, CommandFactory};
use crate::error::Result;

/// LiNaStore Server CLI
#[derive(Parser)]
#[command(version, about, long_about = None)]
struct ServerCli {
    #[command(subcommand)]
    command: Option<ServerCommands>,
}

#[derive(Subcommand, Clone)]
enum ServerCommands {
    /// Start the LiNaStore server
    Start(StartArgs),
    /// Stop the LiNaStore server
    Stop(StopArgs),
}

/// Arguments for the start command
#[derive(Parser, Clone)]
struct StartArgs {
    /// Run server in foreground (do not daemonize)
    #[arg(long = "foreground")]
    foreground: bool,

    /// Directory to store log files (default: linastore/logs)
    #[arg(long = "log-dir")]
    log_dir: Option<String>,
}

/// Arguments for the stop command
#[derive(Parser, Clone)]
struct StopArgs {
    /// Force stop the server (kill if graceful shutdown fails)
    #[arg(short = 'f', long = "force")]
    force: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = ServerCli::parse();

    match &cli.command {
        Some(ServerCommands::Start(args)) => {
            // Write PID file
            // Run the server
            utils::run_server(args.log_dir.clone(), !args.foreground).await
        }
        Some(ServerCommands::Stop(args)) => utils::handle_stop(args.force),
        None => {
            // No subcommand provided: show help
            let mut cmd = ServerCli::command();
            cmd.print_help()?;
            println!();
            Ok(())
        }
    }
}
