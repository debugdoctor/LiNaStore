use clap::{Parser, Subcommand, ArgAction};

/// Arguments for the list command
///
/// This command lists files stored in LiNaStore with various filtering options
#[derive(Parser, Clone)]
pub struct ListArgs {
    #[arg(short = 'n', long = "num", value_name = "NUMBER", default_value_t = 50,
        help = "Number of items to list (0 for unlimited, default: 50)")]
    pub n: u64,

    #[arg(short = 'e', long = "ext", value_name = "EXTENSION",
        help = "Filter by file extension (e.g., 'txt', 'jpg')")]
    pub isext: Option<String>,

    #[arg(value_name = "Pattern",
        help = "Search pattern (supports wildcards like '*.txt')")]
    pub input_files: Option<String>,
}

/// Arguments for the put command
///
/// This command stores files into LiNaStore with various options
#[derive(Parser, Clone)]
pub struct PutArgs {
    #[arg(
        short = 'l',
        action = ArgAction::SetTrue,
        help = "List files after storing them successfully"
    )]
    pub list: bool,

    #[arg(
        short = 'z',
        action = ArgAction::SetTrue,
        help = "Compress files before storing to save space"
    )]
    pub compressed: bool,

    #[arg(
        short = 'c',
        action = ArgAction::SetTrue,
        help = "Overwrite existing files with same name"
    )]
    pub cover: bool,

    #[arg(value_name = "FILES",
        help = "Files to store (can specify multiple files)")]
    pub input_files: Vec<String>,
}

/// Arguments for the get command
///
/// This command retrieves files from LiNaStore
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
    
    #[arg(value_name = "FILES",
        help = "Files to retrieve (can specify multiple files)")]
    pub input_files: Vec<String>,
}


/// Arguments for the delete command
///
/// This command deletes files from LiNaStore
#[derive(Parser, Clone)]
pub struct DeleteArgs {
    #[arg(value_name = "PATTERN",
        help = "Pattern of files to delete (supports wildcards, use with caution)")]
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

/// Arguments for the tidy command
///
/// This command organizes files by removing duplicates and creating symbolic links
#[derive(Parser, Clone)]
pub struct TidyArgs {
    #[arg(
        long = "keep-new",
        action = ArgAction::SetTrue,
        help = "Keep newer files when duplicates are found (default: keep older)"
    )]
    pub keep_new: bool,

    #[arg(
        value_name = "DIR",
        default_value = &".",
        help = "Target directory to tidy (default: current directory)"
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