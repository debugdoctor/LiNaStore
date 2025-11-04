mod libs;
extern crate linabase;

use std::env;
use clap::Parser;
use libs::{handler, command::{Cli, Commands}};
use std::process;

use crate::libs::command::{FileArgs, StoreArgs};

/// Main entry point for LiNaStore CLI application
///
/// This function handles the initialization and command routing for the LiNaStore system.
/// It includes improved error handling with graceful exits and meaningful error messages.
fn main() {
    // Parse command line arguments with proper error handling
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            eprintln!("Error parsing command line arguments: {}", e);
            process::exit(1);
        }
    };

    // Get current directory with enhanced error handling
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

    // Execute the appropriate command with error handling
    let result = match &cli.commands {
        Some(Commands::Storage(StoreArgs::List(args))) => {
            handler::handle_list(&current_dir, args)
        },
        Some(Commands::Storage(StoreArgs::Put(args))) => {
            handler::handle_put(&current_dir, args)
        },
        Some(Commands::Storage(StoreArgs::Get(args))) => {
            handler::handle_get(&current_dir, args)
        },
        Some(Commands::Storage(StoreArgs::Delete(args))) => {
            handler::handle_delete(&current_dir, args)
        },
        Some(Commands::File(FileArgs::Tidy(args))) => {
            handler::handle_tidy(args)
        },
        None => {
            eprintln!("Error: No command provided. Use --help for usage information.");
            process::exit(1);
        }
    };

    // Handle any errors that occurred during command execution
    if let Err(e) = result {
        eprintln!("Error: {}", e);
        process::exit(1);
    }
}