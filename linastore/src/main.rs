mod command;
mod handler;

use clap::Parser;
use command::{Cli, Commands};
use std::env;
use std::process;

use command::{FileArgs, StoreArgs};

#[tokio::main]
async fn main() {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            eprintln!("Error parsing command line arguments: {}", e);
            process::exit(1);
        }
    };

    let current_dir = match env::current_dir() {
        Ok(path) => match path.to_str() {
            Some(dir_str) => dir_str.to_string(),
            None => {
                eprintln!("Error: Current directory contains invalid UTF-8 characters");
                process::exit(1);
            }
        },
        Err(e) => {
            eprintln!("Error: Failed to get current directory: {}", e);
            process::exit(1);
        }
    };

    let result = match &cli.commands {
        Some(Commands::Storage(StoreArgs::List(args))) => handler::handle_list(&current_dir, args).await,
        Some(Commands::Storage(StoreArgs::Put(args))) => handler::handle_put(&current_dir, args).await,
        Some(Commands::Storage(StoreArgs::Get(args))) => handler::handle_get(&current_dir, args).await,
        Some(Commands::Storage(StoreArgs::Delete(args))) => {
            handler::handle_delete(&current_dir, args).await
        }
        Some(Commands::File(FileArgs::Tidy(args))) => handler::handle_tidy(args),
        None => {
            eprintln!("Error: No command provided. Use --help for usage information.");
            process::exit(1);
        }
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        process::exit(1);
    }
}
