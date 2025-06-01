use clap::{Parser, Subcommand, ArgAction};

#[derive(Parser, Clone)]
pub struct ListArgs {
    #[arg(short = 'n', long = "num", value_name = "NUMBER", default_value_t = 100, 
        help = "Number of items to list (0 for unlimited, default: 100)")]
    pub n: u32,

    #[arg(short = 'e', long = "ext", value_name = "EXTENSION")]
    pub isext: Option<String>,

    #[arg(value_name = "Pattern")]
    pub input_files: Option<String>,
}

#[derive(Parser, Clone)]
pub struct PutArgs {
    #[arg(short = 'l', action = ArgAction::SetTrue)]
    pub list: bool,

    #[arg(short = 'z', action = ArgAction::SetTrue)]
    pub compressed: bool,

    #[arg(short = 'c', action = ArgAction::SetTrue)]
    pub cover: bool,

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