use clap::{Parser, Subcommand};

/// Arguments for the mount command
#[derive(Parser, Clone)]
pub struct MountArgs {
    #[arg(value_name = "MOUNT_POINT", help = "Mount point directory")]
    pub mount_point: String,

    #[arg(
        short = 'r',
        long = "root",
        value_name = "DIR",
        default_value = ".",
        help = "Storage root directory (default: current directory)"
    )]
    pub root: String,

    #[arg(
        short = 'f',
        long = "foreground",
        action = clap::ArgAction::SetTrue,
        help = "Run in foreground (default: daemonize)"
    )]
    pub foreground: bool,

    #[arg(
        short = 'z',
        long = "compressed",
        action = clap::ArgAction::SetTrue,
        help = "Store file content compressed (default: uncompressed)"
    )]
    pub compressed: bool,
}

/// Arguments for the umount command
#[derive(Parser, Clone)]
pub struct UmountArgs {
    #[arg(value_name = "MOUNT_POINT", help = "Mount point to unmount")]
    pub mount_point: String,
}

#[derive(Subcommand, Clone)]
pub enum Commands {
    #[command(about = "Mount linastore as a FUSE filesystem")]
    Mount(MountArgs),
    #[command(about = "Unmount a linastore FUSE filesystem")]
    Umount(UmountArgs),
}

#[derive(Parser)]
#[command(version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub commands: Option<Commands>,
}
