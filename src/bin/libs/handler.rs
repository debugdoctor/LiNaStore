use linabase::service::{ StoreManager, TidyManager };
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

    let store_manager = match StoreManager::new(root) {
        Ok(store_manager) => store_manager,
        Err(e) => {
            eprintln!("Failed to list files: {}", e);
            return;
        }
    };

    let file_names = match store_manager.list(&pattern, args.n + 1, isext, true){
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

    for file_index in 0..file_names.len() as u64 {
        if file_index < args.n {
            println!("{}", file_names[file_index as usize].name);
        } else {
            println!("...");
            break;
        }
    }
}

pub fn handle_put(root: &str, args: &command::PutArgs){
    let store_manager = match StoreManager::new(root) {
        Ok(store_manager) => store_manager,
        Err(e) => {
            eprintln!("Failed to list files: {}", e);
            return;
        }
    };
    
    match store_manager.put(&args.input_files, args.cover, args.compressed){
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
    let store_manager = match StoreManager::new(root) {
        Ok(store_manager) => store_manager,
        Err(e) => {
            eprintln!("Failed to list files: {}", e);
            return;
        }
    };

    let dest_path=  match fs::canonicalize(&args.dest) {
        Ok(absolute_path) => absolute_path,
        Err(e) => {
            eprintln!("Invalid destination path: {}", e);
            return;
        }
    };

    if args.input_files.len() == 0 {
        eprintln!("No files requested for get");
        return;
    } else if args.input_files.len() == 1 {
        let links = match store_manager.list(&format!("{}*", args.input_files[0]), 0,false, true){
            Ok(links) => links,
            Err(e) => {
                eprintln!("Failed to search files: {}", e);
                return;
            }
        };

        if links.len() == 0 {
            eprintln!("No files found");
            return;
        } else if links.len() == 1 {
            if links[0].name  == args.input_files[0] {
                if let Err(e) = store_manager.get_and_save(&args.input_files, dest_path) {
                    eprintln!("Failed to get files: {}", e);
                    return;
                }
            } else {
                eprintln!("No files found");
                return;
            }
        } else {
            eprintln!("Multiple files found");
            for link in &links {
                println!(" {}", link.name);
            }
            return;
        }
    }

    println!("Files get successfully");
}

pub fn handle_delete(root: &str, args: &command::DeleteArgs){
    let pattern = args.input_files.clone().unwrap_or_else(|| String::from(""));
    let store_manager = match StoreManager::new(root) {
        Ok(store_manager) => store_manager,
        Err(e) => {
            eprintln!("Failed to list files: {}", e);
            return;
        }
    };

    if let Err(e) = store_manager.delete(&pattern, true) {
        eprintln!("Failed to delete files: {}", e);
        return;
    }

    println!("Files deleted successfully");
}

pub fn handle_tidy(args: &command::TidyArgs){
    let mut tidy_manager = TidyManager::new();

    if let Err(e) = tidy_manager.tidy(&args.target_dir, args.keep_new) {
        eprintln!("Failed to tidy: {}", e);
        return;
    }
}