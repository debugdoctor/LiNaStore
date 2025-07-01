use chrono::{DateTime, Utc};
use nanoid;
use std::{
    collections::HashMap,
    error::Error,
    fs, io,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    result::Result,
};

use crate::utils::BlockManager;

use super::dao::{Dao, Link, Source};
use super::utils;

const NANOID_MAP: [char; 62] = [
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i',
    'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z', 'A', 'B',
    'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O', 'P', 'Q', 'R', 'S', 'T', 'U',
    'V', 'W', 'X', 'Y', 'Z',
];

#[derive(Debug)]
pub struct StoreManager {
    root: PathBuf,
    dao: Dao,
    bm: BlockManager,
}

pub struct TidyManager {
    map_cache: HashMap<String, Vec<(PathBuf, String)>>,
}

impl StoreManager {
    pub fn new<P: AsRef<Path>>(root: P) -> Result<Self, Box<dyn Error>> {
        let root_path = root.as_ref().to_path_buf(); // Convert to owning type
        fs::create_dir_all(root_path.join("linadata"))?;

        Ok(StoreManager {
            root: root_path.clone(), // Store owned path
            dao: Dao::new(root_path.join("linadata").join("meta.db"))?,
            bm: BlockManager::new(),
        })
    }

    pub fn list(
        &self,
        pattern: &str,
        n: u64,
        isext: bool,
        use_regex: bool,
    ) -> Result<Vec<Link>, Box<dyn Error>> {
        let links = if isext {
            self.dao.get_links_by_ext(pattern)?
        } else if (pattern == "" || pattern == "*") && use_regex {
            self.dao.get_n_links(n)?
        } else if pattern.contains('*') && use_regex {
            let sql_pattern = pattern.replace('*', "%");
            self.dao.get_links_by_name(&sql_pattern, true)?
        } else {
            self.dao.get_links_by_name(pattern, false)?
        };

        Ok(links)
    }

    pub fn get_binary_data(&self, file_name: &str) -> Result<Vec<u8>, Box<dyn Error>> {
        if file_name.is_empty() {
            return Err(Box::new(io::Error::new(
                io::ErrorKind::Other,
                "No filename provided",
            )));
        }
        let links = self.dao.get_links_by_name(file_name, false)?;
        let link = links
            .get(0)
            .ok_or_else(|| Box::new(io::Error::new(io::ErrorKind::NotFound, "File not found")))?;

        let source = self
            .dao
            .get_source_by_id(&link.source_id)?
            .ok_or_else(|| Box::new(io::Error::new(io::ErrorKind::NotFound, "File not found")))?;

        let source_path = self
            .root
            .join("linadata")
            .join(&source.id[0..4])
            .join(&source.id[4..6])
            .join(&source.id);

        Ok(if source.compressed {
            self.bm.decompress_all(&fs::read(&source_path)?, source.size as usize)?
        } else {
            fs::read(&source_path)?
        })
    }

    pub fn get_and_save<P: AsRef<Path>>(
        &self,
        files: &Vec<String>,
        dest: P,
    ) -> Result<(), Box<dyn Error>> {
        if files.is_empty() {
            return Err(Box::new(io::Error::new(
                io::ErrorKind::Other,
                "No files requested",
            )));
        }
        // Create destination directory once
        fs::create_dir_all(dest.as_ref())?;

        for file in files {
            let file_name = file;
            let links = self.dao.get_links_by_name(file_name, false)?;

            let link = links.get(0).ok_or_else(|| {
                Box::new(io::Error::new(io::ErrorKind::NotFound, "File not found"))
            })?;

            let source = self.dao.get_source_by_id(&link.source_id)?.ok_or_else(|| {
                Box::new(io::Error::new(io::ErrorKind::NotFound, "File not found"))
            })?;

            let source_path = self
                .root
                .join("linadata")
                .join(&source.id[0..4])
                .join(&source.id[4..6])
                .join(&source.id);

            let dest_path = dest.as_ref().to_path_buf().join(&link.name);

            if source.compressed {
                let data = self.bm.decompress_all(&fs::read(&source_path)?, source.size as usize)?;
                fs::write(&dest_path, data)?;
            } else {
                fs::copy(&source_path, &dest_path)?;
            }
        }

        Ok(())
    }

    pub fn put_binary_data(
        &self,
        file_name: &str,
        input: &Vec<u8>,
        cover: bool,
        compressed: bool,
    ) -> Result<(), Box<dyn Error>> {
        if file_name.is_empty() {
            return Err(Box::new(io::Error::new(
                io::ErrorKind::Other,
                "No filename provided",
            )));
        }

        let links = self.dao.get_links_by_name(file_name, false)?;
        let data_path = self.root.join("linadata");

        if links.len() > 0 {
            let link = links.get(0).ok_or_else(|| {
                Box::new(io::Error::new(io::ErrorKind::NotFound, "File not found"))
            })?;

            let hash256 = utils::get_hash256_from_binary(input);
            let source = self.dao.get_source_by_id(&link.source_id)?.ok_or_else(|| {
                Box::new(io::Error::new(io::ErrorKind::NotFound, "Source not found"))
            })?;

            let new_size = input.len() as u64;

            if cover {
                // Update hash256 and source compression and size
                self.dao.update_source(
                    &link.source_id,
                    &hash256,
                    compressed,
                    new_size,
                    source.count,
                )?;
                let target_file = data_path
                    .join(&link.source_id[..4])
                    .join(&link.source_id[4..6])
                    .join(&link.source_id);

                if compressed {
                    let data = self.bm.compress_all(input)?;
                    fs::write(target_file, data)?;
                } else {
                    fs::write(target_file, input)?;
                }
            } else {
                if hash256 == source.hash256 && source.compressed == compressed {
                    return Ok(());
                }

                // 1. Source Release
                let source_count = source
                    .count
                    .checked_sub(1)
                    .ok_or(io::Error::new(io::ErrorKind::Other, "Source count is 0"))?;

                self.release_source(&link, &source, source_count)?;

                // 2. Insert new source
                let id = Self::file_name_gen();
                self.dao
                    .insert_source(&id, &hash256, compressed, new_size)?;
                self.dao.update_link_source_id(&link.id, &id)?;
                let target_file = data_path.join(&id[..4]).join(&id[4..6]).join(&id);
                let _ = fs::write(target_file, input)?;
            }
        } else {
            // Check hash256
            let hash256 = utils::get_hash256_from_binary(input);
            let ext = Path::new(&file_name)
                .extension()
                .unwrap_or_default()
                .to_str()
                .unwrap_or("")
                .to_string();

            // If hash256 exists, count + 1
            if let Some(source) = self.dao.get_source_by_hash256(&hash256)? {
                self.dao.insert_link(file_name, &ext, &source.id)?;
                // Update source count
                return Ok(self.dao.update_source(
                    &source.id,
                    &source.hash256,
                    source.compressed,
                    source.size,
                    source.count + 1,
                )?);
            }

            let id = Self::file_name_gen();
            let size = input.len() as u64;
            // Create source directory
            let source_dir = data_path.join(&id[..4]).join(&id[4..6]);
            fs::create_dir_all(&source_dir)?;

            self.dao.insert_source(&id, &hash256, compressed, size)?;
            self.dao.insert_link(file_name, &ext, &id)?;

            let target_file = source_dir.join(&id);

            if compressed {
                let data = self.bm.compress_all(input)?;
                fs::write(target_file, data)?;
            } else {
                fs::write(target_file, input)?;
            }
        }
        Ok(())
    }

    pub fn put(
        &self,
        files: &Vec<String>,
        cover: bool,
        compressed: bool,
    ) -> Result<(), Box<dyn Error>> {
        if files.is_empty() {
            return Err(Box::new(io::Error::new(
                io::ErrorKind::Other,
                "No files requested",
            )));
        }

        for file in files {
            if !fs::exists(&file)? {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("File {} not found", &file),
                )));
            }

            let file_path = Path::new(&file);
            let file_name = file_path
                .file_name()
                .ok_or_else(|| {
                    Box::new(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Invalid file path format",
                    ))
                })?
                .to_str()
                .ok_or_else(|| {
                    Box::new(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "File name contains invalid UTF-8 characters",
                    ))
                })?;

            let links = self.dao.get_links_by_name(file_name, false)?;
            let data_path = self.root.join("linadata");

            if links.len() > 0 {
                let link = links.get(0).ok_or_else(|| {
                    Box::new(io::Error::new(io::ErrorKind::NotFound, "File not found"))
                })?;

                let hash256 = utils::get_hash256_from_file(file_path)?;
                let source = self.dao.get_source_by_id(&link.source_id)?.ok_or_else(|| {
                    Box::new(io::Error::new(io::ErrorKind::NotFound, "Source not found"))
                })?;

                let new_size = fs::metadata(&file)?.size();

                if cover {
                    // Update hash256 and source compression and size
                    self.dao.update_source(
                        &link.source_id,
                        &hash256,
                        compressed,
                        new_size,
                        source.count,
                    )?;
                    let target_file = data_path
                        .join(&link.source_id[..4])
                        .join(&link.source_id[4..6])
                        .join(&link.source_id);

                    if compressed {
                        let input = fs::read(file_path)?;
                        let data = self.bm.compress_all(&input)?;
                        fs::write(target_file, data)?;
                    } else {
                        fs::copy(&file, target_file)?;
                    }
                } else {
                    if hash256 == source.hash256 && source.compressed == compressed {
                        return Ok(());
                    }

                    // 1. Source Release
                    let source_count = source
                        .count
                        .checked_sub(1)
                        .ok_or(io::Error::new(io::ErrorKind::Other, "Source count is 0"))?;

                    self.release_source(&link, &source, source_count)?;

                    // 2. Insert new source
                    let id = Self::file_name_gen();
                    self.dao
                        .insert_source(&id, &hash256, compressed, new_size)?;
                    self.dao.update_link_source_id(&link.id, &id)?;
                    let target_file = data_path.join(&id[..4]).join(&id[4..6]).join(&id);
                    fs::copy(&file, target_file)?;
                }
            } else {
                // Check hash256
                let hash256 = utils::get_hash256_from_file(file_path)?;
                let ext = Path::new(&file)
                    .extension()
                    .unwrap_or_default()
                    .to_str()
                    .unwrap_or("")
                    .to_string();

                // If hash256 exists, count + 1
                if let Some(source) = self.dao.get_source_by_hash256(&hash256)? {
                    self.dao.insert_link(file_name, &ext, &source.id)?;
                    // Update source count
                    return Ok(self.dao.update_source(
                        &source.id,
                        &source.hash256,
                        source.compressed,
                        source.size,
                        source.count + 1,
                    )?);
                }

                let id = Self::file_name_gen();
                let size = fs::metadata(&file)?.len();
                // Create source directory
                let source_dir = data_path.join(&id[..4]).join(&id[4..6]);
                fs::create_dir_all(&source_dir)?;

                self.dao.insert_source(&id, &hash256, compressed, size)?;
                self.dao.insert_link(file_name, &ext, &id)?;

                if compressed {
                    let input = fs::read(file_path)?;
                    let data = self.bm.compress_all(&input)?;
                    fs::write(source_dir.join(&id), data)?;
                } else {
                    fs::copy(file, source_dir.join(&id))?;
                }
            }
        }
        Ok(())
    }

    pub fn delete(&self, pattern: &str, use_regx: bool) -> Result<(), Box<dyn Error>> {
        if pattern == "" {
            return Err(Box::new(io::Error::new(
                io::ErrorKind::Other,
                "No files requested",
            )));
        }

        let links = Self::list(&self, pattern, 0, false, use_regx)?;
        for link in links {
            let source = self.dao.get_source_by_id(&link.source_id)?.ok_or_else(|| {
                Box::new(io::Error::new(io::ErrorKind::NotFound, "File not found"))
            })?;

            self.dao.delete_link_by_id(&link.id)?;
            let source_count = source
                .count
                .checked_sub(1)
                .ok_or(io::Error::new(io::ErrorKind::Other, "Source count is 0"))?;

            if source_count == 0 {
                self.release_source(&link, &source, source_count)?;
            }
        }

        Ok(())
    }

    fn release_source(
        &self,
        link: &Link,
        source: &Source,
        source_count: u64,
    ) -> Result<(), Box<dyn Error>> {
        // Delete source if count is 0
        if source_count > 0 {
            self.dao.update_source(
                &source.id,
                &source.hash256,
                source.compressed,
                source.size,
                source_count as u64,
            )?;
        } else {
            let source_path = self
                .root
                .join("linadata")
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

impl TidyManager {
    pub fn new() -> Self {
        TidyManager {
            map_cache: HashMap::with_capacity(0x8000),
        }
    }

    pub fn tidy<P: AsRef<Path>>(
        &mut self,
        target_path: P,
        keep_new: bool,
    ) -> Result<(), Box<dyn Error>> {
        let paths = utils::path_walk(target_path)?;

        for path in paths {
            self.file_info_collector(&path);
        }

        for key in self.map_cache.keys() {
            let file_infos = match self.map_cache.get(key) {
                Some(files) if !files.is_empty() => files,
                _ => continue,
            };

            let target_file_info = if keep_new {
                self.find_extreme_file(file_infos, |a, b| a > b)
            } else {
                self.find_extreme_file(file_infos, |a, b| a < b)
            };

            for file_info in file_infos {
                if file_info.1 != *target_file_info.1 && file_info.0 != *target_file_info.0 {
                    let relative_file_path =
                        self.relative_path_with_same_root(&file_info.0, target_file_info.0);

                    match fs::remove_file(&file_info.0) {
                        Ok(_) => {}
                        Err(_) => {
                            eprintln!("Failed to tidy with file: {}", relative_file_path.display());
                            continue;
                        }
                    }
                    utils::create_symlink(relative_file_path, &file_info.0)?;
                    // Result output visible for users
                    println!(
                        "{} -> {}",
                        file_info.0.display(),
                        target_file_info.0.display()
                    );
                }
            }
        }

        Ok(())
    }

    fn file_info_collector(&mut self, path: &Path) {
        let hash_code = match utils::get_hash256_from_file(path) {
            Ok(hash_code) => hash_code,
            Err(e) => panic!(
                "Hash of file {} generate error: {}",
                path.display(),
                e.to_string()
            ),
        };

        let created_date = match fs::metadata(path) {
            Ok(metadata) => match metadata.created() {
                Ok(date) => date,
                Err(e) => panic!(
                    "Get file {} create date error: {}",
                    path.display(),
                    e.to_string()
                ),
            },
            Err(e) => panic!(
                "Get file {} metadata error: {}",
                path.display(),
                e.to_string()
            ),
        };

        let formated_created_date = DateTime::<Utc>::from(created_date)
            .format("%Y%m%d%H%M%S")
            .to_string();

        self.map_cache
            .entry(hash_code)
            .or_insert_with(Vec::new)
            .push((path.to_path_buf(), formated_created_date));
    }

    fn find_extreme_file<'a, F>(
        &self,
        file_infos: &'a [(PathBuf, String)],
        compare: F,
    ) -> (&'a PathBuf, &'a String)
    where
        F: Fn(&String, &String) -> bool,
    {
        let mut extreme = (&file_infos[0].0, &file_infos[0].1);
        for file_info in &file_infos[1..] {
            if compare(&file_info.1, extreme.1) {
                extreme = (&file_info.0, &file_info.1);
            }
        }
        extreme
    }

    fn relative_path_with_same_root<P: AsRef<Path>>(&self, from: P, to: P) -> PathBuf {
        let from_components: Vec<_> = from.as_ref().components().collect();
        let to_components: Vec<_> = to.as_ref().components().collect();
        let min_len = from_components.len().min(to_components.len());
        let mut common = 0;

        let mut result = PathBuf::with_capacity(0x10);

        while common < min_len && from_components[common] == to_components[common] {
            common += 1;
        }

        if from_components.len() - common > 1 {
            for _ in &from_components[common + 1..] {
                result.push("..");
            }
        } else if from_components.len() - common == 1 {
            result.push(".");
        }

        for comp in &to_components[common..] {
            result.push(comp.as_os_str());
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use rand::Rng;

    use super::*;
    fn generate_random_binary(size: usize) -> Vec<u8> {
        let mut rng = rand::rng();
        let mut data = vec![0u8; size];
        rng.fill(&mut data[..]);
        data
    }

    #[test]
    fn test_data_flow_store() {
        let data = generate_random_binary(64 * 1024);
        let sm = StoreManager::new(".").unwrap();
        let _ = sm.put_binary_data("random.txt", &data, true, true);
        let data_get = sm.get_binary_data("random.txt").unwrap();
        assert_eq!(data, data_get, "Data flow test failed");
    }
}
