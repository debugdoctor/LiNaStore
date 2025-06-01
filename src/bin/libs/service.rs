use std::{ error::Error, fs, io, os::unix::fs::MetadataExt, path::{Path, PathBuf} };
use chrono::Utc;
use nanoid;

use super::dao::{Dao, Link, Source};
use super::utils;

const NANOID_MAP: [char; 62] = [
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9',
    'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z',
    'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O', 'P', 'Q', 'R', 'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z'
    ];

#[derive(Debug, Clone)]
pub struct Manager {
    root: PathBuf,
    dao: Dao
}

impl Manager {
    pub fn new<P: AsRef<Path>>(root: P) -> Result<Self, Box<dyn Error>> {
        let root_path = root.as_ref().to_path_buf(); // Convert to owning type
        fs::create_dir_all(root_path.join("lihadata"))?;
      
        Ok(Manager {
            root: root_path.clone(), // Store owned path
            dao: Dao::new(
                root_path.join("lihadata").join("meta.db")
            )?
        })
    }

    pub fn list(&self, pattern: &str, n: u32, isext: bool) -> Result<Vec<Link>, Box<dyn Error>> {
        let links = if isext {
                self.dao.get_links_by_ext(pattern)?
            } else if pattern == "" || pattern == "*" {
                self.dao.get_n_links(n)?
            } else if pattern.contains('*') {
                let sql_pattern = pattern.replace('*', "%");
                self.dao.get_links_by_name(&sql_pattern, true)?
            } else {
                self.dao.get_links_by_name(pattern, false)?
            };
        
        Ok(links)
    }

    pub fn get_and_save<P: AsRef<Path>>(&self, files: &Vec<String>, dest: P) -> Result<(), Box<dyn Error>> {
        if files.is_empty() {
            return Err(Box::new(io::Error::new(io::ErrorKind::Other, "No files requested")));
        }
        // Create destination directory once
        fs::create_dir_all(dest.as_ref())?;

        for file in files {
            let file_name = file;
            let links = self.dao.get_links_by_name(file_name, false)?;

            let link = links.get(0).ok_or_else(|| 
                Box::new(io::Error::new(io::ErrorKind::NotFound, "File not found"))
            )?;

            let source = self.dao.get_source_by_id(&link.source_id)?
                .ok_or_else(|| Box::new(io::Error::new(io::ErrorKind::NotFound, "File not found")))?;

            let source_path = self.root.join("lihadata")
                .join(&source.id[0..4])
                .join(&source.id[4..6])
                .join(&source.id);

            let dest_path = dest.as_ref().to_path_buf().join(&link.name);
            
            if source.compressed {
                let bm = utils::BlockManager::new();
                let data =  bm.decompress_all(&fs::read(&source_path)?,  source.size as usize)?;
                fs::write(&dest_path, data)?;
            } else {
                fs::copy(&source_path, &dest_path)?;
            }
        }

        Ok(())
    }

    pub fn put(&self, files: &Vec<String>, cover: bool, compressed: bool) -> Result<(), Box<dyn Error>> {
        if files.is_empty() {
            return Err(Box::new(io::Error::new(io::ErrorKind::Other, "No files requested")));
        }

        for file in files {
            if !fs::exists(&file)?{
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("File {} not found", &file),
                )));
            }

            let file_path = Path::new(&file);
            let file_name = file_path.file_name()
                .ok_or_else(|| Box::new(io::Error::new(
                    io::ErrorKind::InvalidInput, 
                    "Invalid file path format"
                )))?
                .to_str()
                .ok_or_else(|| Box::new(io::Error::new(
                    io::ErrorKind::InvalidInput, 
                    "File name contains invalid UTF-8 characters"
                )))?;

            let links = self.dao.get_links_by_name(file_name, false)?;
            let data_path = self.root.join("lihadata");

            if links.len() > 0 {
                let link = links.get(0).ok_or_else(|| 
                    Box::new(io::Error::new(io::ErrorKind::NotFound, "File not found"))
                )?;

                let hash256 = utils::get_hash256(file_path)?;
                let source = self.dao.get_source_by_id(&link.source_id)?
                    .ok_or_else(|| Box::new(io::Error::new(io::ErrorKind::NotFound, "Source not found")))?;

                let new_size = fs::metadata(&file)?.size();

                if cover {
                    // Update hash256 and source compression and size
                    self.dao.update_source(&link.source_id, &hash256, compressed, new_size, source.count)?;
                    let target_file = data_path
                        .join(&link.source_id[..4])
                        .join(&link.source_id[4..6])
                        .join(&link.source_id);

                    if compressed {
                        let bm = utils::BlockManager::new();
                        let input = fs::read(file_path)?;
                        let data = bm.compress_all(&input)?;
                        fs::write(target_file, data)?;
                    } else {
                        fs::copy(&file, target_file)?;
                    }
                    
                } else {
                    if hash256 == source.hash256 && source.compressed == compressed {
                        return Ok(());
                    }

                    // 1. Source Release
                    let source_count = source.count.checked_sub(1).ok_or(
                    io::Error::new(io::ErrorKind::Other, "Source count is 0")
                    )?;

                    self.release_source(&link, &source, source_count)?;

                    // 2. Insert new source
                    let id = Self::file_name_gen();
                    self.dao.insert_source(&id, &hash256, compressed, new_size)?;
                    self.dao.update_link_source_id(&link.id, &id)?;
                    let target_file = data_path
                        .join(&id[..4])
                        .join(&id[4..6])
                        .join(&id);
                    fs::copy(&file, target_file)?;
                }
            } else {
                // Check hash256
                let hash256 = utils::get_hash256(file_path)?;
                let ext = Path::new(&file)
                    .extension()
                    .unwrap_or_default()
                    .to_str()
                    .unwrap_or("")
                    .to_string();

                // If hash256 exists, count + 1
                if let Some(source) = self.dao.get_source_by_hash256(&hash256)?{
                    self.dao.insert_link(file_name, &ext, &source.id)?;
                    // Update source count
                    return Ok(self.dao.update_source(
                        &source.id,
                        &source.hash256,
                        source.compressed,
                        source.size,
                        source.count + 1)?);
                }

                let id = Self::file_name_gen();
                let size = fs::metadata(&file)?.len();
                // Create source directory
                let source_dir = data_path.join(&id[..4]).join(&id[4..6]);
                fs::create_dir_all(&source_dir)?;

                self.dao.insert_source(&id, &hash256, compressed, size)?;
                self.dao.insert_link(file_name, &ext, &id)?;

                if compressed {
                    let bm = utils::BlockManager::new();
                    let input = fs::read(file_path)?;
                    let data = bm.compress_all(&input)?;
                    fs::write(source_dir.join(&id), data)?;
                } else {
                    fs::copy(file, source_dir.join(&id))?;
                }
            }
        }
        Ok(())
    }

    pub fn delete(&self, file: &str) -> Result<(), Box<dyn Error>>{
        if file == "" {
            return Err(Box::new(io::Error::new(io::ErrorKind::Other, "No files requested")));
        }

        let links = Self::list(&self, file, 0,false)?;
        for link in links {
            let source = self.dao.get_source_by_id(&link.source_id)?
                .ok_or_else(|| Box::new(io::Error::new(io::ErrorKind::NotFound, "File not found")))?;

            self.dao.delete_link_by_id(&link.id)?;
            let source_count = source.count.checked_sub(1).ok_or(
            io::Error::new(io::ErrorKind::Other, "Source count is 0")
            )?;

            self.release_source(&link, &source, source_count)?;

            if source_count == 0 { break; }
        }

        Ok(())
    }

    fn release_source(&self, link: &Link, source: &Source, source_count: u64) -> Result<(), Box<dyn Error>> {
        // Delete source if count is 0
        if source_count > 0 {
            self.dao.update_source(
                &source.id,
                &source.hash256,
                source.compressed,
                source.size,
                source_count as u64
            )?;
        } else {
            let source_path = self.root.join("lihadata")
                .join(&link.source_id[..4])
                .join(&link.source_id[4..6])
                .join(&link.source_id);

            self.dao.delete_source_by_id(&source.id)?;
            fs::remove_file(source_path)?;
        }
        Ok(())
    }

    fn file_name_gen() -> String {
        let utc_time = Utc::now();
        let utc_time_formated = utc_time.format("%Y%m%d%H%M%S").to_string();

        let nano_id = nanoid::nanoid!(8, &NANOID_MAP);

        format!("{}{}", utc_time_formated, nano_id)
    }
}