mod libs;
extern crate linabase;

use std::env;
use clap::Parser;
use libs::{handler, command::{Cli, Commands}};

use crate::libs::command::{FileArgs, StoreArgs};

fn main() {
  let cli = Cli::parse();
  let binding = env::current_dir()
      .unwrap_or_else(|_| panic!("Failed to get current directory"));
  let current_dir = binding
      .to_str()
      .unwrap_or_else(|| panic!("Failed to convert current directory to string"));

  match &cli.commands {
        Some(Commands::Storage(StoreArgs::List(args))) => handler::handle_list(&current_dir, args),
        Some(Commands::Storage(StoreArgs::Put(args))) => handler::handle_put(&current_dir, args),
        Some(Commands::Storage(StoreArgs::Get(args))) => handler::handle_get(&current_dir, args),
        Some(Commands::Storage(StoreArgs::Delete(args))) => handler::handle_delete(&current_dir, args),
        Some(Commands::File(FileArgs::Tidy(args))) => handler::handle_tidy(args),
        None => println!("No command provided"),
    }
}