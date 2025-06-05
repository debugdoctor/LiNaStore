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
    #[arg(
        short = 'l',
        action = ArgAction::SetTrue,
        help = "List files after storing"
    )]
    pub list: bool,

    #[arg(
        short = 'z',
        action = ArgAction::SetTrue,
        help = "Compress files before storing"
    )]
    pub compressed: bool,

    #[arg(
        short = 'c',
        action = ArgAction::SetTrue,
        help = "Overwrite existing files"
    )]
    pub cover: bool,

    #[arg(value_name = "FILES")]
    pub input_files: Vec<String>,
}

#[derive(Parser, Clone)]
pub struct GetArgs {
    #[arg(
        short = 'd',
        long = "dest",
        value_name = "DIR",
        default_value = &".",
        help = "Destination directory (default: current directory)"
    )]
    pub dest: String,
    
    #[arg(value_name = "FILES")]
    pub input_files: Vec<String>,
}


#[derive(Parser, Clone)]
pub struct DeleteArgs {
    #[arg(value_name = "FILES")]
    pub input_files: Option<String>,
}

#[derive(Subcommand, Clone)]
pub enum StoreArgs {
    #[command(
        about = "List files in linastore"
    )]
    List(ListArgs),

    #[command(
        about = "Store files into linastore"
    )]
    Put(PutArgs),
    
    #[command(
        about = "Retrieve files from linastore"
    )]
    Get(GetArgs),

    #[command(
        about = "Delete files from linastore"
    )]
    Delete(DeleteArgs),
}

#[derive(Parser, Clone)]
pub struct TidyArgs {
    #[arg(
        long = "keep-new",
        action = ArgAction::SetTrue,
        help = "Keep new files (default: false)"
    )]
    pub keep_new: bool,

    // #[arg(
    //     short = 'e',
    //     long = "ext",
    //     value_name = "EXTENSION",
    //     help = "The files with these extensions will be tidied"
    // )]
    // pub ext: Vec<String>,

    #[arg(
        value_name = "DIR",
        default_value = &".",
        help = "Target directory (default: current directory)"
    )]
    pub target_dir: String,
}

#[derive(Subcommand, Clone)]
pub enum FileArgs {
    #[command(
        about = "Linastore file system tools", 
    )]
    Tidy(TidyArgs),
}

// Update Commands enum
#[derive(Subcommand, Clone)]
pub enum Commands {
    #[command(
        subcommand,
        about = "Linastore storage operations", 
    )]
    Storage(StoreArgs),

    #[command(
        subcommand,
        about = "Linastore file system operations", 
    )]
    File(FileArgs),
}

// Fix Cli struct
#[derive(Parser)]
#[command(version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub commands: Option<Commands>,
}