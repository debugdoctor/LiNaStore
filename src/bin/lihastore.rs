mod lib;

use std::env;
use clap::Parser;
use lib::{handler, command::{Cli, Commands}};
fn main() {
  let cli = Cli::parse();
  let binding = env::current_dir()
      .expect("Failed to get current directory");
  let current_dir = binding
      .to_str()
      .unwrap_or_else(|| panic!("Failed to convert current directory to string"));

  match &cli.commands {
        Some(Commands::List(args)) => handler::handle_list(&current_dir, args),
        Some(Commands::Put(args)) => handler::handle_put(&current_dir, args),
        Some(Commands::Get(args)) => handler::handle_get(&current_dir, args),
        Some(Commands::Delete(args)) => handler::handle_delete(&current_dir, args),
        None => println!("No command provided"),
    }
}