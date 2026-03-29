use bytes::Bytes;
use chrono::{DateTime, Utc};
use nanoid;
use std::{
    collections::HashMap,
    error::Error,
    fs, io,
    path::{Path, PathBuf},
    result::Result,
    sync::Arc,
};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::utils::BlockManager;

use super::dao::{Dao, Link, Source};
use super::utils;

type BoxError = Box<dyn Error + Send + Sync>;

const NANOID_MAP: [char; 62] = [
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i',
    'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z', 'A', 'B',
    'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O', 'P', 'Q', 'R', 'S', 'T', 'U',
    'V', 'W', 'X', 'Y', 'Z',
];

fn boxed_io_error(kind: io::ErrorKind, message: impl Into<String>) -> BoxError {
    Box::new(io::Error::new(kind, message.into()))
}

fn dao_to_io_error(err: anyhow::Error) -> io::Error {
    io::Error::other(err.to_string())
}

#[derive(Debug)]
pub struct StoreManager {
    root: PathBuf,
    dao: Dao,
    bm: BlockManager,
    operation_lock: Arc<RwLock<()>>,
}

pub struct TidyManager {
    map_cache: HashMap<String, Vec<(PathBuf, String)>>,
}

// Constructor and query-oriented APIs.
impl StoreManager {
    pub async fn new<P: AsRef<Path>>(root: P) -> Result<Self, BoxError> {
        let root_path = root.as_ref().to_path_buf(); // Convert to owning type
        fs::create_dir_all(root_path.join("linadata"))?;

        Ok(StoreManager {
            root: root_path.clone(), // Store owned path
            dao: Dao::new(root_path.join("linadata").join("meta.db"))
                .await
                .map_err(dao_to_io_error)?,
            bm: BlockManager::new(),
            operation_lock: Arc::new(RwLock::new(())),
        })
    }

    pub async fn list(
        &self,
        pattern: &str,
        n: u64,
        isext: bool,
        use_regex: bool,
    ) -> Result<Vec<Link>, BoxError> {
        let _read_guard = self.operation_lock.read().await;
        self.list_locked(pattern, n, isext, use_regex).await
    }
}

// Read and write storage APIs.
impl StoreManager {
    pub async fn get_binary_data(&self, file_name: &str) -> Result<Bytes, BoxError> {
        if file_name.is_empty() {
            return Err(boxed_io_error(io::ErrorKind::Other, "No filename provided"));
        }

        let (compressed, source_size, file_bytes) = {
            let _read_guard = self.operation_lock.read().await;
            let links = self
                .dao
                .get_links_by_name(file_name, false)
                .await
                .map_err(dao_to_io_error)?;
            let link = links
                .get(0)
                .ok_or_else(|| boxed_io_error(io::ErrorKind::NotFound, "File not found"))?;

            let source = self
                .dao
                .get_source_by_id(&link.source_id)
                .await
                .map_err(dao_to_io_error)?
                .ok_or_else(|| boxed_io_error(io::ErrorKind::NotFound, "File not found"))?;

            let source_path = self
                .root
                .join("linadata")
                .join(&source.id[0..4])
                .join(&source.id[4..6])
                .join(&source.id);

            let file_bytes = fs::read(&source_path)?;
            (source.compressed, source.size as usize, file_bytes)
        };

        Ok(if compressed {
            Bytes::from(self.bm.decompress_all(&file_bytes, source_size)?)
        } else {
            Bytes::from(file_bytes)
        })
    }

    pub async fn get_and_save<P: AsRef<Path>>(
        &self,
        files: &Vec<String>,
        dest: P,
    ) -> Result<(), BoxError> {
        if files.is_empty() {
            return Err(boxed_io_error(io::ErrorKind::Other, "No files requested"));
        }
        let dest_root = dest.as_ref().to_path_buf();
        fs::create_dir_all(&dest_root)?;

        for file in files {
            let data = self.get_binary_data(file).await?;
            let file_name = Path::new(file)
                .file_name()
                .ok_or_else(|| {
                    boxed_io_error(io::ErrorKind::InvalidInput, "Invalid file name for save target")
                })?;
            let dest_path = dest_root.join(file_name);
            if let Some(parent) = dest_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&dest_path, data)?;
        }

        Ok(())
    }

    pub async fn put_binary_data(
        &self,
        file_name: &str,
        input: &Bytes,
        cover: bool,
        compressed: bool,
    ) -> Result<(), BoxError> {
        if file_name.is_empty() {
            return Err(boxed_io_error(io::ErrorKind::Other, "No filename provided"));
        }

        let new_hash256 = utils::get_hash256_from_binary(input);
        let new_size = input.len() as u64;
        let new_storage_bytes = self.encode_source_data(input, compressed)?;
        let ext = Path::new(&file_name)
            .extension()
            .unwrap_or_default()
            .to_str()
            .unwrap_or("")
            .to_string();

        let _write_guard = self.operation_lock.write().await;
        self.put_binary_data_locked(
            file_name,
            cover,
            compressed,
            &new_hash256,
            new_size,
            &new_storage_bytes,
            &ext,
        )
            .await
    }

    pub async fn put(
        &self,
        files: &Vec<String>,
        cover: bool,
        compressed: bool,
    ) -> Result<(), BoxError> {
        if files.is_empty() {
            return Err(boxed_io_error(io::ErrorKind::Other, "No files requested"));
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
                    boxed_io_error(io::ErrorKind::InvalidInput, "Invalid file path format")
                })?
                .to_str()
                .ok_or_else(|| {
                    boxed_io_error(
                        io::ErrorKind::InvalidInput,
                        "File name contains invalid UTF-8 characters",
                    )
                })?;
            let input = Bytes::from(fs::read(file_path)?);
            self.put_binary_data(file_name, &input, cover, compressed).await?;
        }
        Ok(())
    }

    pub async fn delete(&self, pattern: &str, use_regx: bool) -> Result<(), BoxError> {
        if pattern == "" {
            return Err(boxed_io_error(io::ErrorKind::Other, "No files requested"));
        }

        {
            let _write_guard = self.operation_lock.write().await;
            let links = self.list_locked(pattern, 0, false, use_regx).await?;
            for link in links {
                let source = self
                    .dao
                    .get_source_by_id(&link.source_id)
                    .await
                    .map_err(dao_to_io_error)?
                    .ok_or_else(|| boxed_io_error(io::ErrorKind::NotFound, "File not found"))?;

                let source_count = source
                    .count
                    .checked_sub(1)
                    .ok_or(io::Error::new(io::ErrorKind::Other, "Source count is 0"))?;

                self.dao.delete_link_by_id(&link.id).await?;
                if let Err(err) = self.release_source(&link, &source, source_count).await {
                    let ext = Path::new(&link.name)
                        .extension()
                        .unwrap_or_default()
                        .to_str()
                        .unwrap_or("")
                        .to_string();
                    let _ = self
                        .dao
                        .insert_link_with_id(&link.id, &link.name, &ext, &link.source_id)
                        .await
                        .map_err(dao_to_io_error);
                    return Err(err);
                }
            }
        }

        Ok(())
    }
}

// Source lifecycle and consistency helpers.
impl StoreManager {
    async fn list_locked(
        &self,
        pattern: &str,
        n: u64,
        isext: bool,
        use_regex: bool,
    ) -> Result<Vec<Link>, BoxError> {
        let links = if isext {
            self.dao.get_links_by_ext(pattern).await.map_err(dao_to_io_error)?
        } else if (pattern == "" || pattern == "*") && use_regex {
            self.dao.get_n_links(n).await.map_err(dao_to_io_error)?
        } else if pattern.contains('*') && use_regex {
            let sql_pattern = pattern.replace('*', "%");
            self.dao
                .get_links_by_name(&sql_pattern, true)
                .await
                .map_err(dao_to_io_error)?
        } else {
            self.dao
                .get_links_by_name(pattern, false)
                .await
                .map_err(dao_to_io_error)?
        };

        Ok(links)
    }

    async fn put_binary_data_locked(
        &self,
        file_name: &str,
        cover: bool,
        compressed: bool,
        new_hash256: &str,
        new_size: u64,
        new_storage_bytes: &[u8],
        ext: &str,
    ) -> Result<(), BoxError> {
        let links = self
            .dao
            .get_links_by_name(file_name, false)
            .await
            .map_err(dao_to_io_error)?;

        if links.len() > 0 {
            let link = links
                .get(0)
                .ok_or_else(|| boxed_io_error(io::ErrorKind::NotFound, "File not found"))?;

            let source = self
                .dao
                .get_source_by_id(&link.source_id)
                .await
                .map_err(dao_to_io_error)?
                .ok_or_else(|| boxed_io_error(io::ErrorKind::NotFound, "Source not found"))?;

            if cover {
                let source_path = self.source_path(&link.source_id);
                let previous_storage_bytes = fs::read(&source_path)?;

                self.persist_source_bytes(&link.source_id, new_storage_bytes)?;
                if let Err(err) = self
                    .dao
                    .update_source(
                        &link.source_id,
                        new_hash256,
                        compressed,
                        new_size,
                        source.count,
                    )
                    .await
                {
                    let _ = self.persist_source_bytes(&link.source_id, &previous_storage_bytes);
                    return Err(Box::new(dao_to_io_error(err)));
                }
            } else {
                if new_hash256 == source.hash256 && source.compressed == compressed {
                    return Ok(());
                }

                let new_source_id = Self::file_name_gen();
                self.persist_source_bytes(&new_source_id, new_storage_bytes)?;

                if let Err(err) = self
                    .dao
                    .insert_source(&new_source_id, new_hash256, compressed, new_size)
                    .await
                {
                    let _ = self.remove_source_file_if_exists(&new_source_id);
                    return Err(Box::new(dao_to_io_error(err)));
                }

                if let Err(err) = self.dao.update_link_source_id(&link.id, &new_source_id).await {
                    let _ = self
                        .dao
                        .delete_source_by_id(&new_source_id)
                        .await
                        .map_err(dao_to_io_error);
                    let _ = self.remove_source_file_if_exists(&new_source_id);
                    return Err(Box::new(dao_to_io_error(err)));
                }

                let source_count = source
                    .count
                    .checked_sub(1)
                    .ok_or(io::Error::new(io::ErrorKind::Other, "Source count is 0"))?;

                if let Err(err) = self.release_source(link, &source, source_count).await {
                    let _ = self
                        .dao
                        .update_link_source_id(&link.id, &source.id)
                        .await
                        .map_err(dao_to_io_error);
                    let _ = self
                        .dao
                        .delete_source_by_id(&new_source_id)
                        .await
                        .map_err(dao_to_io_error);
                    let _ = self.remove_source_file_if_exists(&new_source_id);
                    return Err(Box::new(io::Error::other(err.to_string())));
                }
            }
        } else {
            if let Some(source) = self
                .dao
                .get_source_by_hash256(new_hash256)
                .await
                .map_err(dao_to_io_error)?
            {
                let link_id = Uuid::new_v4().to_string();
                self.dao
                    .insert_link_with_id(&link_id, file_name, ext, &source.id)
                    .await
                    .map_err(dao_to_io_error)?;

                if let Err(err) = self
                    .dao
                    .update_source(
                        &source.id,
                        &source.hash256,
                        source.compressed,
                        source.size,
                        source.count + 1,
                    )
                    .await
                {
                    let _ = self.dao.delete_link_by_id(&link_id).await.map_err(dao_to_io_error);
                    return Err(Box::new(dao_to_io_error(err)));
                }

                return Ok(());
            }

            let source_id = Self::file_name_gen();
            let link_id = Uuid::new_v4().to_string();

            self.persist_source_bytes(&source_id, new_storage_bytes)?;

            if let Err(err) = self
                .dao
                .insert_source(&source_id, new_hash256, compressed, new_size)
                .await
            {
                let _ = self.remove_source_file_if_exists(&source_id);
                return Err(Box::new(dao_to_io_error(err)));
            }

            if let Err(err) = self
                .dao
                .insert_link_with_id(&link_id, file_name, ext, &source_id)
                .await
            {
                let _ = self
                    .dao
                    .delete_source_by_id(&source_id)
                    .await
                    .map_err(dao_to_io_error);
                let _ = self.remove_source_file_if_exists(&source_id);
                return Err(Box::new(dao_to_io_error(err)));
            }
        }

        Ok(())
    }

    async fn release_source(
        &self,
        link: &Link,
        source: &Source,
        source_count: u64,
    ) -> Result<(), BoxError> {
        // Delete source if count is 0
        if source_count > 0 {
            self.dao
                .update_source(
                    &source.id,
                    &source.hash256,
                    source.compressed,
                    source.size,
                    source_count as u64,
                )
                .await
                .map_err(dao_to_io_error)?;
        } else {
            let source_path = self.source_path(&link.source_id);
            let source_dir = self.source_dir(&link.source_id);
            let tombstone_path = source_dir.join(format!("{}.deleting", link.source_id));

            fs::rename(&source_path, &tombstone_path)?;

            if let Err(err) = self.dao.delete_source_by_id(&source.id).await {
                let _ = fs::rename(&tombstone_path, &source_path);
                return Err(Box::new(dao_to_io_error(err)));
            }

            if let Err(err) = fs::remove_file(&tombstone_path) {
                let _ = self
                    .dao
                    .insert_source(
                        &source.id,
                        &source.hash256,
                        source.compressed,
                        source.size,
                    )
                    .await;
                let _ = fs::rename(&tombstone_path, &source_path);
                return Err(Box::new(err));
            }
        }
        Ok(())
    }
}

// Filesystem and identifier helpers.
impl StoreManager {
    fn file_name_gen() -> String {
        let utc_time = Utc::now();
        let utc_time_formated = utc_time.format("%Y%m%d%H%M%S").to_string();

        let nano_id = nanoid::nanoid!(8, &NANOID_MAP);

        format!("{}{}", utc_time_formated, nano_id)
    }

    fn source_dir(&self, source_id: &str) -> PathBuf {
        self.root
            .join("linadata")
            .join(&source_id[..4])
            .join(&source_id[4..6])
    }

    fn source_path(&self, source_id: &str) -> PathBuf {
        self.source_dir(source_id).join(source_id)
    }

    fn encode_source_data(
        &self,
        input: &Bytes,
        compressed: bool,
    ) -> Result<Vec<u8>, BoxError> {
        if compressed {
            Ok(self.bm.compress_all(input)?)
        } else {
            Ok(input.to_vec())
        }
    }

    fn persist_source_bytes(&self, source_id: &str, bytes: &[u8]) -> Result<(), BoxError> {
        let source_dir = self.source_dir(source_id);
        fs::create_dir_all(&source_dir)?;

        let target_path = source_dir.join(source_id);
        let tmp_path = source_dir.join(format!("{}.tmp-{}", source_id, Uuid::new_v4()));

        fs::write(&tmp_path, bytes)?;
        fs::rename(&tmp_path, &target_path)?;

        Ok(())
    }

    fn remove_source_file_if_exists(&self, source_id: &str) -> Result<(), BoxError> {
        let source_path = self.source_path(source_id);
        match fs::remove_file(source_path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(Box::new(err)),
        }
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
    ) -> Result<(), BoxError> {
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
    use std::sync::Arc;
    use tempfile::TempDir;

    use super::*;

    fn generate_random_binary(size: usize) -> Bytes {
        let mut rng = rand::rng();
        let mut data = vec![0u8; size];
        rng.fill(&mut data[..]);
        Bytes::from(data)
    }

    #[tokio::test]
    async fn test_data_flow_store() {
        let data = generate_random_binary(64 * 1024);
        let sm = StoreManager::new(".").await.unwrap();
        let _ = sm.put_binary_data("random.txt", &data, true, true).await;
        let data_get = sm.get_binary_data("random.txt").await.unwrap();
        assert_eq!(data, data_get, "Data flow test failed");
    }

    #[tokio::test]
    async fn test_store_manager_new() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let sm = StoreManager::new(temp_dir.path()).await;
        assert!(sm.is_ok());

        // Verify linadata directory was created
        let linadata_path = temp_dir.path().join("linadata");
        assert!(linadata_path.exists());
    }

    #[tokio::test]
    async fn test_put_binary_data_new_file() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let sm = StoreManager::new(temp_dir.path()).await.expect("Failed to create StoreManager");
        let data = Bytes::from(vec![1, 2, 3, 4, 5]);

        let result = sm.put_binary_data("test.txt", &data, false, false).await;
        assert!(result.is_ok());

        // Verify file can be retrieved
        let retrieved = sm.get_binary_data("test.txt").await.expect("Failed to get data");
        assert_eq!(data, retrieved);
    }

    #[tokio::test]
    async fn test_put_binary_data_compressed() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let sm = StoreManager::new(temp_dir.path()).await.expect("Failed to create StoreManager");
        let data = Bytes::from(vec![42u8; 10000]); // Highly compressible data

        let result = sm.put_binary_data("compressed.txt", &data, false, true).await;
        assert!(result.is_ok());

        let retrieved = sm
            .get_binary_data("compressed.txt")
            .await
            .expect("Failed to get data");
        assert_eq!(data, retrieved);
    }

    #[tokio::test]
    async fn test_put_binary_data_cover() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let sm = StoreManager::new(temp_dir.path()).await.expect("Failed to create StoreManager");
        let data1 = Bytes::from(vec![1, 2, 3, 4, 5]);
        let data2 = Bytes::from(vec![6, 7, 8, 9, 10]);

        // Put initial data
        sm.put_binary_data("test.txt", &data1, false, false)
            .await
            .expect("Failed to put data");

        // Cover with new data
        sm.put_binary_data("test.txt", &data2, true, false)
            .await
            .expect("Failed to cover data");

        let retrieved = sm.get_binary_data("test.txt").await.expect("Failed to get data");
        assert_eq!(data2, retrieved);
    }

    #[tokio::test]
    async fn test_put_binary_data_empty_filename() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let sm = StoreManager::new(temp_dir.path()).await.expect("Failed to create StoreManager");
        let data = Bytes::from(vec![1, 2, 3, 4, 5]);

        let result = sm.put_binary_data("", &data, false, false).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_get_binary_data_not_found() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let sm = StoreManager::new(temp_dir.path()).await.expect("Failed to create StoreManager");

        let result = sm.get_binary_data("nonexistent.txt").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_get_binary_data_empty_filename() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let sm = StoreManager::new(temp_dir.path()).await.expect("Failed to create StoreManager");

        let result = sm.get_binary_data("").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_list_all_files() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let sm = StoreManager::new(temp_dir.path()).await.expect("Failed to create StoreManager");
        let data1 = Bytes::from(vec![1, 2, 3]);
        let data2 = Bytes::from(vec![4, 5, 6]);

        sm.put_binary_data("file1.txt", &data1, false, false)
            .await
            .expect("Failed to put data");
        sm.put_binary_data("file2.txt", &data2, false, false)
            .await
            .expect("Failed to put data");

        let links = sm.list("", 0, false, true).await.expect("Failed to list files");
        assert_eq!(links.len(), 2);
    }

    #[tokio::test]
    async fn test_list_by_name() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let sm = StoreManager::new(temp_dir.path()).await.expect("Failed to create StoreManager");
        let data = Bytes::from(vec![1, 2, 3]);

        sm.put_binary_data("test_file.txt", &data, false, false)
            .await
            .expect("Failed to put data");

        let links = sm
            .list("test_file.txt", 0, false, false)
            .await
            .expect("Failed to list files");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].name, "test_file.txt");
    }

    #[tokio::test]
    async fn test_list_by_extension() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let sm = StoreManager::new(temp_dir.path()).await.expect("Failed to create StoreManager");
        let data1 = Bytes::from(vec![1, 2, 3]);
        let data2 = Bytes::from(vec![4, 5, 6]);
        let data3 = Bytes::from(vec![7, 8, 9]);

        sm.put_binary_data("file1.txt", &data1, false, false)
            .await
            .expect("Failed to put data");
        sm.put_binary_data("file2.txt", &data2, false, false)
            .await
            .expect("Failed to put data");
        sm.put_binary_data("file3.pdf", &data3, false, false)
            .await
            .expect("Failed to put data");

        let txt_links = sm
            .list("txt", 0, true, false)
            .await
            .expect("Failed to list files");
        assert_eq!(txt_links.len(), 2);

        let pdf_links = sm
            .list("pdf", 0, true, false)
            .await
            .expect("Failed to list files");
        assert_eq!(pdf_links.len(), 1);
    }

    #[tokio::test]
    async fn test_list_with_limit() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let sm = StoreManager::new(temp_dir.path()).await.expect("Failed to create StoreManager");
        let data = Bytes::from(vec![1, 2, 3]);

        for i in 0..5 {
            let filename = format!("file{}.txt", i);
            sm.put_binary_data(&filename, &data, false, false)
                .await
                .expect("Failed to put data");
        }

        let links = sm.list("", 3, false, true).await.expect("Failed to list files");
        assert_eq!(links.len(), 3);
    }

    #[tokio::test]
    async fn test_delete_file() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let sm = StoreManager::new(temp_dir.path()).await.expect("Failed to create StoreManager");
        let data = Bytes::from(vec![1, 2, 3]);

        sm.put_binary_data("test.txt", &data, false, false)
            .await
            .expect("Failed to put data");

        // Verify file exists
        let links = sm
            .list("test.txt", 0, false, false)
            .await
            .expect("Failed to list files");
        assert_eq!(links.len(), 1);

        // Delete file
        sm.delete("test.txt", false).await.expect("Failed to delete file");

        // Verify file is deleted
        let links_after = sm
            .list("test.txt", 0, false, false)
            .await
            .expect("Failed to list files");
        assert!(links_after.is_empty());
    }

    #[tokio::test]
    async fn test_delete_deduplicated_file_decrements_source_count() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let sm = StoreManager::new(temp_dir.path()).await.expect("Failed to create StoreManager");
        let data = Bytes::from(vec![9, 8, 7, 6]);

        sm.put_binary_data("file1.txt", &data, false, false)
            .await
            .expect("Failed to put first file");
        sm.put_binary_data("file2.txt", &data, false, false)
            .await
            .expect("Failed to put second file");

        let links_before = sm
            .dao
            .get_links_by_name("file1.txt", false)
            .await
            .expect("Failed to get file1 links");
        let source_id = links_before[0].source_id.clone();

        let source_before = sm
            .dao
            .get_source_by_id(&source_id)
            .await
            .expect("Failed to get source before delete")
            .expect("Expected source before delete");
        assert_eq!(source_before.count, 2);

        sm.delete("file1.txt", false)
            .await
            .expect("Failed to delete file1");

        let source_after = sm
            .dao
            .get_source_by_id(&source_id)
            .await
            .expect("Failed to get source after delete")
            .expect("Expected source after delete");
        assert_eq!(source_after.count, 1);

        let remaining_data = sm
            .get_binary_data("file2.txt")
            .await
            .expect("Failed to read remaining deduplicated file");
        assert_eq!(remaining_data, data);
    }

    #[tokio::test]
    async fn test_delete_empty_pattern() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let sm = StoreManager::new(temp_dir.path()).await.expect("Failed to create StoreManager");

        let result = sm.delete("", false).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_get_and_save() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let sm = StoreManager::new(temp_dir.path()).await.expect("Failed to create StoreManager");
        let save_dir = TempDir::new().expect("Failed to create save dir");
        let data = Bytes::from(vec![1, 2, 3, 4, 5]);

        sm.put_binary_data("test.txt", &data, false, false)
            .await
            .expect("Failed to put data");

        let files = vec!["test.txt".to_string()];
        sm.get_and_save(&files, save_dir.path())
            .await
            .expect("Failed to get and save");

        let saved_path = save_dir.path().join("test.txt");
        assert!(saved_path.exists());

        let saved_data = std::fs::read(&saved_path).expect("Failed to read saved file");
        assert_eq!(&data[..], &saved_data);
    }

    #[tokio::test]
    async fn test_get_and_save_empty_files() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let sm = StoreManager::new(temp_dir.path()).await.expect("Failed to create StoreManager");
        let save_dir = TempDir::new().expect("Failed to create save dir");
        let files: Vec<String> = vec![];

        let result = sm.get_and_save(&files, save_dir.path()).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_file_name_gen() {
        let name1 = StoreManager::file_name_gen();
        let name2 = StoreManager::file_name_gen();

        // Names should be different
        assert_ne!(name1, name2);

        // Names should be 22 characters (14 for timestamp + 8 for nanoid)
        assert_eq!(name1.len(), 22);
        assert_eq!(name2.len(), 22);
    }

    #[tokio::test]
    async fn test_deduplication_same_content() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let sm = StoreManager::new(temp_dir.path()).await.expect("Failed to create StoreManager");
        let data = Bytes::from(vec![1, 2, 3, 4, 5]);

        // Put same data with different names
        sm.put_binary_data("file1.txt", &data, false, false)
            .await
            .expect("Failed to put data");
        sm.put_binary_data("file2.txt", &data, false, false)
            .await
            .expect("Failed to put data");

        // Both should retrieve same data
        let data1 = sm.get_binary_data("file1.txt").await.expect("Failed to get data");
        let data2 = sm.get_binary_data("file2.txt").await.expect("Failed to get data");
        assert_eq!(data1, data2);
        assert_eq!(data1, data);
    }

    #[tokio::test]
    async fn test_concurrent_puts_preserve_dedup_count() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let sm = Arc::new(
            StoreManager::new(temp_dir.path())
                .await
                .expect("Failed to create StoreManager"),
        );
        let data = Bytes::from(vec![1, 3, 5, 7, 9]);

        let sm1 = Arc::clone(&sm);
        let sm2 = Arc::clone(&sm);
        let data1 = data.clone();
        let data2 = data.clone();

        let (res1, res2) = tokio::join!(
            async move { sm1.put_binary_data("concurrent1.txt", &data1, false, false).await },
            async move { sm2.put_binary_data("concurrent2.txt", &data2, false, false).await },
        );

        assert!(res1.is_ok(), "first concurrent write failed: {:?}", res1.err());
        assert!(res2.is_ok(), "second concurrent write failed: {:?}", res2.err());

        let link1 = sm
            .dao
            .get_links_by_name("concurrent1.txt", false)
            .await
            .expect("Failed to query concurrent1 link");
        let link2 = sm
            .dao
            .get_links_by_name("concurrent2.txt", false)
            .await
            .expect("Failed to query concurrent2 link");

        assert_eq!(link1.len(), 1);
        assert_eq!(link2.len(), 1);
        assert_eq!(link1[0].source_id, link2[0].source_id);

        let source = sm
            .dao
            .get_source_by_id(&link1[0].source_id)
            .await
            .expect("Failed to query shared source")
            .expect("Shared source should exist");
        assert_eq!(source.count, 2);
    }

    #[tokio::test]
    async fn test_large_file() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let sm = StoreManager::new(temp_dir.path()).await.expect("Failed to create StoreManager");
        let data = generate_random_binary(1024 * 1024); // 1MB

        sm.put_binary_data("large.txt", &data, false, false)
            .await
            .expect("Failed to put data");

        let retrieved = sm.get_binary_data("large.txt").await.expect("Failed to get data");
        assert_eq!(data, retrieved);
    }

    #[test]
    fn test_tidy_manager_new() {
        let tm = TidyManager::new();
        assert!(tm.map_cache.is_empty());
    }

    #[test]
    fn test_relative_path_with_same_root() {
        let tm = TidyManager::new();

        // Test same directory
        let result = tm.relative_path_with_same_root("/a/b/c.txt", "/a/b/d.txt");
        assert_eq!(result, PathBuf::from("./d.txt"));

        // Test parent directory
        let result = tm.relative_path_with_same_root("/a/b/c.txt", "/a/d.txt");
        assert_eq!(result, PathBuf::from("../d.txt"));
    }
}
