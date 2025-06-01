use super::service::Manager;
use super::command;
use std::fs;
use std::path::Path;

pub fn handle_list(root: &str, args: &command::ListArgs){
    let pattern: String;
    let isext: bool;
    
    if args.isext == None {
        pattern = args.input_files.clone().unwrap_or_else(|| String::from("*"));
        isext = false;
    } else {
        pattern = match &args.isext{
            Some(ext) => {
                isext = true;
                ext.to_string()
            },
            None => {
                eprintln!("Failed to list files with empty extension");
                return;
            },
        };
    }

    let manager = match Manager::new(root) {
        Ok(manager) => manager,
        Err(e) => {
            eprintln!("Failed to list files: {}", e);
            return;
        }
    };

    let file_names = match manager.list(&pattern, args.n, isext){
        Ok(file_names) => file_names,
        Err(e) => {
            eprintln!("Failed to list files: {}", e);
            return;
        }
    };

    if file_names.is_empty() {
        println!("No files found");
        return;
    }

    for file_name in &file_names {
        println!("{}", file_name.name);
    }

    if args.n != 0 && file_names.len() > args.n as usize {
        println!("...");
    }
}

pub fn handle_put(root: &str, args: &command::PutArgs){
    let manager = match Manager::new(root) {
        Ok(manager) => manager,
        Err(e) => {
            eprintln!("Failed to list files: {}", e);
            return;
        }
    };
    
    match manager.put(&args.input_files, args.cover, args.compressed){
        Ok(_) => {},
        Err(e) => {
            eprintln!("Failed to store files: {}", e);
            return;
        }
    };

    if args.list {
        println!("Files stored successfully:");
        for file in &args.input_files {
            println!("{}", 
                Path::new(file)
                    .file_name()
                    .and_then(|os| os.to_str())
                    .unwrap_or("<unknown>")
            );
        }
    } else {
        println!("Files stored successfully");
    }
}

pub fn handle_get(root: &str, args: &command::GetArgs){
    let manager = match Manager::new(root) {
        Ok(manager) => manager,
        Err(e) => {
            eprintln!("Failed to list files: {}", e);
            return;
        }
    };

    let dest_path= match &args.dest {
        Some(path) => {
            // Convert to absolute path
            match fs::canonicalize(path) {
                Ok(absolute_path) => absolute_path,
                Err(e) => {
                    eprintln!("Invalid destination path: {}", e);
                    return;
                }
            }
        },
        None => {
            eprintln!("Destination path is required");
            return;
        }
    };
    
    if let Err(e) = manager.get_and_save(&args.input_files, dest_path) {
        eprintln!("Failed to retrieve and download files: {}", e);
        return;
    }

    println!("Files retrieve and download successfully");
}

pub fn handle_delete(root: &str, args: &command::DeleteArgs){
    let pattern = args.input_files.clone().unwrap_or_else(|| String::from(""));
    let manager = match Manager::new(root) {
        Ok(manager) => manager,
        Err(e) => {
            eprintln!("Failed to list files: {}", e);
            return;
        }
    };

    if let Err(e) = manager.delete(&pattern) {
        eprintln!("Failed to delete files: {}", e);
        return;
    }

    println!("Files deleted successfully");
}