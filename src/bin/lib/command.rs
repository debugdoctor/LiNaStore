use clap::{Parser, Subcommand};

#[derive(Parser, Clone)]
pub struct ListArgs {
    #[arg(short = 'e', long = "ext", value_name = "EXTENSION")]
    pub isext: Option<String>,

    #[arg(value_name = "Pattern")]
    pub input_files: Option<String>,
}

#[derive(Parser, Clone)]
pub struct PutArgs {    
    #[arg(value_name = "FILES")]
    pub input_files: Vec<String>,
}

#[derive(Parser, Clone)]
pub struct GetArgs {
    #[arg(short = 'd', long = "dest", value_name = "DIR")]
    pub dest: Option<String>,
    
    #[arg(value_name = "FILES")]
    pub input_files: Vec<String>,
}

#[derive(Parser, Clone)]
pub struct DeleteArgs {
    #[arg(value_name = "FILES")]
    pub input_files: Option<String>,
}

// Update Commands enum
#[derive(Subcommand, Clone)]
pub enum Commands {
    #[command(about = "List files in lihastore")]
    List(ListArgs),

    #[command(about = "Store files into lihastore")]
    Put(PutArgs),
    
    #[command(about = "Retrieve files from lihastore")]
    Get(GetArgs),

    #[command(about = "Delete files from lihastore")]
    Delete(DeleteArgs),
}

// Fix Cli struct
#[derive(Parser)]
#[command(version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub commands: Option<Commands>,
}