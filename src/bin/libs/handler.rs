use linabase::service::{ StoreManager, TidyManager };
use super::command;
use std::fs;
use std::path::Path;
use std::error::Error;

/// Handle the list command to display files in the storage
///
/// # Arguments
/// * `root` - The root directory of the storage
/// * `args` - Command line arguments for the list operation
///
/// # Returns
/// * `Result<(), Box<dyn Error>>` - Ok if successful, Err with error details
pub fn handle_list(root: &str, args: &command::ListArgs) -> Result<(), Box<dyn Error>> {
    // Validate input parameters
    if args.n == 0 {
        return Err("Number of items to list cannot be zero".into());
    }

    // Determine search pattern and whether to search by extension
    let (pattern, isext) = if args.isext.is_none() {
        (args.input_files.clone().unwrap_or_else(|| String::from("*")), false)
    } else {
        match &args.isext {
            Some(ext) => (ext.to_string(), true),
            None => return Err("Extension filter cannot be empty".into()),
        }
    };

    // Initialize store manager with error handling
    let store_manager = StoreManager::new(root)
        .map_err(|e| format!("Failed to initialize storage manager: {}", e))?;

    // Retrieve file list with error handling
    let file_names = store_manager.list(&pattern, args.n + 1, isext, true)
        .map_err(|e| format!("Failed to retrieve file list: {}", e))?;

    // Handle empty results
    if file_names.is_empty() {
        println!("No files found");
        return Ok(());
    }

    // Display files with proper pagination
    for (index, file) in file_names.iter().enumerate() {
        if index < args.n as usize {
            println!("{}", file.name);
        } else {
            println!("...");
            break;
        }
    }

    Ok(())
}

/// Handle the put command to store files in the storage
///
/// # Arguments
/// * `root` - The root directory of the storage
/// * `args` - Command line arguments for the put operation
///
/// # Returns
/// * `Result<(), Box<dyn Error>>` - Ok if successful, Err with error details
pub fn handle_put(root: &str, args: &command::PutArgs) -> Result<(), Box<dyn Error>> {
    // Validate input files
    if args.input_files.is_empty() {
        return Err("No files provided for storage".into());
    }

    // Check if all files exist before starting the operation
    for file in &args.input_files {
        if !Path::new(file).exists() {
            return Err(format!("File not found: {}", file).into());
        }
    }

    // Initialize store manager with error handling
    let store_manager = StoreManager::new(root)
        .map_err(|e| format!("Failed to initialize storage manager: {}", e))?;

    // Store files with error handling
    store_manager.put(&args.input_files, args.cover, args.compressed)
        .map_err(|e| format!("Failed to store files: {}", e))?;

    // Display success message with optional file listing
    if args.list {
        println!("Files stored successfully:");
        for file in &args.input_files {
            let file_name = Path::new(file)
                .file_name()
                .and_then(|os| os.to_str())
                .unwrap_or("<invalid filename>");
            println!("  {}", file_name);
        }
    } else {
        println!("Files stored successfully");
    }

    Ok(())
}

/// Handle the get command to retrieve files from the storage
///
/// # Arguments
/// * `root` - The root directory of the storage
/// * `args` - Command line arguments for the get operation
///
/// # Returns
/// * `Result<(), Box<dyn Error>>` - Ok if successful, Err with error details
pub fn handle_get(root: &str, args: &command::GetArgs) -> Result<(), Box<dyn Error>> {
    // Validate input files
    if args.input_files.is_empty() {
        return Err("No files specified for retrieval".into());
    }

    // Initialize store manager with error handling
    let store_manager = StoreManager::new(root)
        .map_err(|e| format!("Failed to initialize storage manager: {}", e))?;

    // Validate and prepare destination directory
    let dest_path = fs::canonicalize(&args.dest)
        .map_err(|e| format!("Invalid destination path '{}': {}", args.dest, e))?;

    // Handle single file retrieval with enhanced logic
    if args.input_files.len() == 1 {
        let file_pattern = format!("{}*", args.input_files[0]);
        let links = store_manager.list(&file_pattern, 0, false, true)
            .map_err(|e| format!("Failed to search for files: {}", e))?;

        match links.len() {
            0 => return Err("No files found matching the specified pattern".into()),
            1 => {
                if links[0].name == args.input_files[0] {
                    store_manager.get_and_save(&args.input_files, &dest_path)
                        .map_err(|e| format!("Failed to retrieve file: {}", e))?;
                } else {
                    return Err("Exact file match not found".into());
                }
            },
            _ => {
                eprintln!("Multiple files found matching pattern:");
                for link in &links {
                    eprintln!("  {}", link.name);
                }
                return Err("Multiple matches found - please specify exact filename".into());
            }
        }
    } else {
        // Handle multiple file retrieval
        store_manager.get_and_save(&args.input_files, &dest_path)
            .map_err(|e| format!("Failed to retrieve files: {}", e))?;
    }

    println!("Files retrieved successfully");
    Ok(())
}

/// Handle the delete command to remove files from the storage
///
/// # Arguments
/// * `root` - The root directory of the storage
/// * `args` - Command line arguments for the delete operation
///
/// # Returns
/// * `Result<(), Box<dyn Error>>` - Ok if successful, Err with error details
pub fn handle_delete(root: &str, args: &command::DeleteArgs) -> Result<(), Box<dyn Error>> {
    // Get deletion pattern with validation
    let pattern = args.input_files.clone().unwrap_or_else(|| String::from(""));
    if pattern.is_empty() {
        return Err("No pattern specified for deletion. This would delete all files.".into());
    }

    // Initialize store manager with error handling
    let store_manager = StoreManager::new(root)
        .map_err(|e| format!("Failed to initialize storage manager: {}", e))?;

    // Perform deletion with error handling
    store_manager.delete(&pattern, true)
        .map_err(|e| format!("Failed to delete files: {}", e))?;

    println!("Files deleted successfully");
    Ok(())
}

/// Handle the tidy command to organize files and remove duplicates
///
/// # Arguments
/// * `args` - Command line arguments for the tidy operation
///
/// # Returns
/// * `Result<(), Box<dyn Error>>` - Ok if successful, Err with error details
pub fn handle_tidy(args: &command::TidyArgs) -> Result<(), Box<dyn Error>> {
    // Validate target directory
    if !Path::new(&args.target_dir).exists() {
        return Err(format!("Target directory does not exist: {}", args.target_dir).into());
    }

    // Initialize tidy manager
    let mut tidy_manager = TidyManager::new();

    // Perform tidy operation with error handling
    tidy_manager.tidy(&args.target_dir, args.keep_new)
        .map_err(|e| format!("Failed to tidy directory: {}", e))?;

    println!("Directory tidied successfully");
    Ok(())
}