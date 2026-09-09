use bytes::Bytes;
use chrono::{DateTime, Utc};
use nanoid;
use std::{
    collections::{HashMap, HashSet},
    error::Error,
    fs as stdfs, io,
    path::{Path, PathBuf},
    result::Result,
    sync::Arc,
    time::Duration,
};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};
use tokio::task;
use uuid::Uuid;

use crate::dbexec::{DbClient, DbExecutor, reconcile_orphans_files};
use crate::utils::BlockManager;

use super::dao::DirEntry;
use super::dao::Link;
#[cfg(test)]
use super::dao::Dao;
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

/// Uploads at or below this size are buffered in memory (avoids temp-file IO
/// and lets dedup skip the disk write entirely); larger ones spool to disk.
const INLINE_MEMORY_THRESHOLD: usize = 4 * 1024 * 1024;

/// Concurrent object store. Metadata lives behind the single-threaded
/// `DbExecutor`; object file IO here is lock-free and runs in parallel.
#[derive(Debug)]
pub struct StoreManager {
    root: PathBuf,
    db: DbClient,
    bm: Arc<BlockManager>,
    #[cfg(test)]
    dao: Dao,
}

/// Streaming reader for a stored object. Yields decompressed data in bounded
/// chunks and verifies the BLAKE3 hash at EOF, so reads never buffer a whole
/// file in memory.
pub struct ObjectReader {
    data_len: u64,
    expected_hash: String,
    hasher: blake3::Hasher,
    inner: ObjectReaderInner,
    eof: bool,
}

enum ObjectReaderInner {
    Plain(tokio::fs::File),
    Blocks {
        file: tokio::fs::File,
        bm: Arc<BlockManager>,
        pending: Vec<u8>,
        pos: usize,
    },
}

impl ObjectReader {
    pub fn data_len(&self) -> u64 {
        self.data_len
    }

    pub async fn read_chunk(&mut self, buf: &mut [u8]) -> Result<usize, BoxError> {
        let n = match &mut self.inner {
            ObjectReaderInner::Plain(file) => file.read(buf).await?,
            ObjectReaderInner::Blocks { .. } => self.read_block_chunk(buf).await?,
        };
        if n > 0 {
            self.hasher.update(&buf[..n]);
        }
        if n == 0 && !self.eof {
            self.eof = true;
            let actual = self.hasher.finalize().to_hex().to_string();
            if actual != self.expected_hash {
                return Err(boxed_io_error(io::ErrorKind::InvalidData, "data integrity check failed"));
            }
        }
        Ok(n)
    }

    async fn read_block_chunk(&mut self, buf: &mut [u8]) -> Result<usize, BoxError> {
        let ObjectReaderInner::Blocks { file, bm, pending, pos } = &mut self.inner else {
            unreachable!();
        };
        if *pos >= pending.len() {
            pending.clear();
            *pos = 0;
            let mut header = [0u8; 3];
            let read = file.read(&mut header).await?;
            if read == 0 {
                return Ok(0);
            }
            if read < 3 {
                file.read_exact(&mut header[read..]).await?;
            }
            let flag = header[0];
            let len = u16::from_le_bytes([header[1], header[2]]) as usize;
            let mut block = vec![0u8; len];
            file.read_exact(&mut block).await?;
            let bm = Arc::clone(bm);
            let decoded = task::spawn_blocking(move || bm.decompress_block(flag, &block)).await??;
            *pending = decoded;
        }
        let take = (pending.len() - *pos).min(buf.len());
        buf[..take].copy_from_slice(&pending[*pos..*pos + take]);
        *pos += take;
        Ok(take)
    }
}

pub struct TidyManager {
    map_cache: HashMap<String, Vec<(PathBuf, String)>>,
}

// Constructor and query-oriented APIs.
impl StoreManager {
    pub async fn new<P: AsRef<Path>>(root: P) -> Result<Self, BoxError> {
        let root_path = root.as_ref().to_path_buf();
        fs::create_dir_all(root_path.join("linadata")).await?;

        // Spawn the DB executor (owns SQLite, runs startup reconciliation).
        let (tx, rx) = mpsc::channel(1024);
        let (ready_tx, ready_rx) = oneshot::channel();
        let spawn_root = root_path.clone();
        tokio::spawn(async move {
            DbExecutor::spawn(spawn_root, rx, ready_tx).await;
        });
        ready_rx
            .await
            .map_err(|_| boxed_io_error(io::ErrorKind::Other, "DB executor failed to start"))??;

        #[cfg(test)]
        let dao = Dao::new(root_path.join("linadata").join("meta.db"))
            .await
            .map_err(|e| boxed_io_error(io::ErrorKind::Other, format!("dao: {}", e)))?;

        Ok(StoreManager {
            root: root_path, // Store owned path
            db: DbClient::new(tx),
            bm: Arc::new(BlockManager::new()),
            #[cfg(test)]
            dao,
        })
    }

    pub async fn list(
        &self,
        pattern: &str,
        n: u64,
        isext: bool,
        use_regex: bool,
    ) -> Result<Vec<Link>, BoxError> {
        self.db.list(pattern, n, isext, use_regex).await
    }

    pub async fn is_dir(&self, path: &str) -> Result<bool, BoxError> {
        self.db.is_dir(path).await
    }

    pub async fn list_child_dirs(&self, parent: &str) -> Result<Vec<DirEntry>, BoxError> {
        self.db.list_child_dirs(parent).await
    }

    pub async fn all_dirs(&self) -> Result<Vec<DirEntry>, BoxError> {
        self.db.all_dirs().await
    }

    pub async fn mkdir(&self, path: &str, parent: &str) -> Result<(), BoxError> {
        self.db.mkdir(path, parent).await
    }

    pub async fn rmdir(&self, path: &str) -> Result<(), BoxError> {
        self.db.rmdir(path).await
    }

    pub async fn set_file_mode(&self, name: &str, mode: u32) -> Result<(), BoxError> {
        self.db.set_file_mode(name, mode).await
    }

    pub async fn set_dir_mode(&self, path: &str, mode: u32) -> Result<(), BoxError> {
        self.db.set_dir_mode(path, mode).await
    }
}

// Read and write storage APIs.
impl StoreManager {
    pub async fn get_binary_data(&self, file_name: &str) -> Result<Bytes, BoxError> {
        if file_name.is_empty() {
            return Err(boxed_io_error(io::ErrorKind::Other, "No filename provided"));
        }

        // Metadata via the DB executor; only the file read runs on this task.
        let meta = self
            .db
            .get_meta(file_name)
            .await?
            .ok_or_else(|| boxed_io_error(io::ErrorKind::NotFound, "File not found"))?;

        let source_path = self.source_path(&meta.source_id);
        let file_bytes = fs::read(&source_path).await?;
        let compressed = meta.compressed;
        let source_size = meta.size as usize;
        let expected_hash = meta.hash256.clone();

        if compressed {
            let bm = Arc::clone(&self.bm);
            let decoded = task::spawn_blocking(move || {
                bm.decompress_all(&file_bytes, source_size)
            })
            .await
            .map_err(|e| boxed_io_error(io::ErrorKind::Other, format!("decompress task join error: {}", e)))??;
            let actual_hash = utils::get_hash256_from_binary(&decoded);
            if actual_hash != expected_hash {
                return Err(boxed_io_error(io::ErrorKind::InvalidData, "data integrity check failed"));
            }
            Ok(Bytes::from(decoded))
        } else {
            let actual_hash = utils::get_hash256_from_binary(&file_bytes);
            if actual_hash != expected_hash {
                return Err(boxed_io_error(io::ErrorKind::InvalidData, "data integrity check failed"));
            }
            Ok(Bytes::from(file_bytes))
        }
    }

    /// Open a streaming read of an object. Returns `None` if the file does not
    /// exist. Data is decompressed on the fly in bounded chunks; the BLAKE3
    /// hash is verified when the stream reaches EOF.
    pub async fn open_read(&self, file_name: &str) -> Result<Option<ObjectReader>, BoxError> {
        if file_name.is_empty() {
            return Err(boxed_io_error(io::ErrorKind::Other, "No filename provided"));
        }
        let meta = match self.db.get_meta(file_name).await? {
            Some(m) => m,
            None => return Ok(None),
        };
        let path = self.source_path(&meta.source_id);
        let file = fs::File::open(&path).await?;
        let inner = if meta.compressed {
            ObjectReaderInner::Blocks {
                file,
                bm: Arc::clone(&self.bm),
                pending: Vec::new(),
                pos: 0,
            }
        } else {
            ObjectReaderInner::Plain(file)
        };
        Ok(Some(ObjectReader {
            data_len: meta.size,
            expected_hash: meta.hash256,
            hasher: blake3::Hasher::new(),
            inner,
            eof: false,
        }))
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
        fs::create_dir_all(&dest_root).await?;

        for file in files {
            let data = self.get_binary_data(file).await?;
            let file_name = Path::new(file)
                .file_name()
                .ok_or_else(|| {
                    boxed_io_error(io::ErrorKind::InvalidInput, "Invalid file name for save target")
                })?;
            let dest_path = dest_root.join(file_name);
            if let Some(parent) = dest_path.parent() {
                fs::create_dir_all(parent).await?;
            }
            fs::write(&dest_path, data).await?;
        }

        Ok(())
    }

    /// Stream a put from a chunk channel with bounded memory and dedup. Small
    /// payloads (known length <= `INLINE_MEMORY_THRESHOLD`) are buffered in
    /// memory; larger/unknown ones are streamed to a temp file on disk. The
    /// BLAKE3 hash is computed incrementally; once the whole body has arrived
    /// the DB is asked to dedup first — on a content-hash hit the object is
    /// merged into the existing source (link only, no file kept).
    pub async fn put_stream(
        &self,
        file_name: &str,
        mut payload: mpsc::Receiver<Bytes>,
        recv_timeout: Duration,
        compressed: bool,
        expected_len: Option<u64>,
        integrity: Option<oneshot::Receiver<Result<(), String>>>,
    ) -> Result<(), BoxError> {
        if file_name.is_empty() {
            return Err(boxed_io_error(io::ErrorKind::Other, "No filename provided"));
        }

        let new_source_id = Self::file_name_gen();
        let source_dir = self.source_dir(&new_source_id);
        let bm = Arc::clone(&self.bm);

        let mut hasher = blake3::Hasher::new();
        let mut total = 0u64;
        // Inline (memory) until the body turns out to be large; spill to a
        // temp file once the buffer would exceed the threshold.
        let mut inline = matches!(expected_len, Some(len) if len as usize <= INLINE_MEMORY_THRESHOLD)
            || expected_len.is_none();
        let mut buf: Vec<u8> = Vec::new();
        let mut spool: Option<(tokio::fs::File, PathBuf)> = None;

        if let Err(e) = Self::collect_body(
            &mut payload,
            recv_timeout,
            &mut hasher,
            &mut total,
            &mut inline,
            &mut buf,
            &mut spool,
            &bm,
            &source_dir,
            &new_source_id,
            compressed,
        )
        .await
        {
            Self::cleanup_spool(spool, &source_dir, &new_source_id).await;
            return Err(e);
        }

        let new_hash256 = hasher.finalize().to_hex().to_string();

        // Size check before any commit.
        if let Some(expected) = expected_len {
            if total != expected {
                Self::cleanup_spool(spool, &source_dir, &new_source_id).await;
                return Err(boxed_io_error(
                    io::ErrorKind::Other,
                    format!("payload size mismatch: expected {} bytes, got {}", expected, total),
                ));
            }
        }

        // Streaming protocol frontends only know their final wire checksum at
        // EOF. Do not dedup or make an object visible until they confirm it.
        if let Some(integrity) = integrity {
            match tokio::time::timeout(recv_timeout, integrity).await {
                Ok(Ok(Ok(()))) => {}
                Ok(Ok(Err(message))) => {
                    Self::cleanup_spool(spool, &source_dir, &new_source_id).await;
                    return Err(boxed_io_error(io::ErrorKind::InvalidData, message));
                }
                Ok(Err(_)) => {
                    Self::cleanup_spool(spool, &source_dir, &new_source_id).await;
                    return Err(boxed_io_error(io::ErrorKind::InvalidData, "payload integrity result dropped"));
                }
                Err(_) => {
                    Self::cleanup_spool(spool, &source_dir, &new_source_id).await;
                    return Err(boxed_io_error(io::ErrorKind::TimedOut, "payload integrity check stalled"));
                }
            }
        }

        // Dedup first: if the content hash already exists, just add a link and
        // skip writing/keeping an object file entirely.
        if self
            .db
            .dedup_or_link(file_name, &new_hash256, compressed)
            .await?
            .is_some()
        {
            Self::cleanup_spool(spool, &source_dir, &new_source_id).await;
            return Ok(());
        }

        // Commit a new object file.
        match (inline, spool) {
            (true, None) => {
                let input = Bytes::from(buf);
                let stored: Bytes = if compressed {
                    let bm = Arc::clone(&self.bm);
                    let input2 = input.clone();
                    let encoded = task::spawn_blocking(move || bm.compress_all(&input2)).await?
                        .map_err(|e| boxed_io_error(io::ErrorKind::Other, format!("compress: {}", e)))?;
                    Bytes::from(encoded)
                } else {
                    input
                };
                self.persist_source_bytes(&new_source_id, &stored).await?;
            }
            (false, Some((file, tmp_path))) => {
                file.sync_all().await?;
                drop(file);
                fs::rename(&tmp_path, &source_dir.join(&new_source_id)).await?;
            }
            _ => return Err(boxed_io_error(io::ErrorKind::Other, "invalid spool state")),
        }

        // Metadata commit — serialized by the DB executor.
        let out = match self
            .db
            .put_meta(file_name, &new_source_id, &new_hash256, compressed, total)
            .await
        {
            Ok(o) => o,
            Err(e) => {
                let _ = self.remove_source_file_if_exists(&new_source_id).await;
                return Err(e);
            }
        };

        if !out.keep_new {
            let _ = self.remove_source_file_if_exists(&new_source_id).await;
        }
        for source_id in &out.released {
            let _ = self.remove_source_file_if_exists(source_id).await;
        }

        Ok(())
    }

    async fn collect_body(
        payload: &mut mpsc::Receiver<Bytes>,
        recv_timeout: Duration,
        hasher: &mut blake3::Hasher,
        total: &mut u64,
        inline: &mut bool,
        buf: &mut Vec<u8>,
        spool: &mut Option<(tokio::fs::File, PathBuf)>,
        bm: &Arc<BlockManager>,
        source_dir: &Path,
        new_source_id: &str,
        compressed: bool,
    ) -> Result<(), BoxError> {
        let block_size = bm.chunk_size();
        // Bytes accumulated but not yet flushed as a full block (spool mode).
        let mut pending: Vec<u8> = Vec::with_capacity(block_size);

        loop {
            let chunk = match tokio::time::timeout(recv_timeout, payload.recv()).await {
                Ok(Some(c)) => c,
                Ok(None) => break,
                Err(_) => return Err(boxed_io_error(io::ErrorKind::Other, "payload stream stalled")),
            };
            hasher.update(&chunk);
            *total += chunk.len() as u64;

            if *inline && buf.len() + chunk.len() <= INLINE_MEMORY_THRESHOLD {
                buf.extend_from_slice(&chunk);
                continue;
            }

            // From here on we are in spool mode: lazily create the temp file,
            // dump the inline buffer into it, and stream blocks as they fill.
            if spool.is_none() {
                let (file, tmp_path) = Self::open_spool(source_dir, new_source_id).await?;
                *spool = Some((file, tmp_path));
            }
            let (file, _) = spool.as_mut().expect("spool just created");
            if *inline {
                if !buf.is_empty() {
                    Self::write_block(file, bm, buf, compressed).await?;
                }
                buf.clear();
                *inline = false;
            }
            pending.extend_from_slice(&chunk);
            while pending.len() >= block_size {
                let block = pending[..block_size].to_vec();
                pending.drain(..block_size);
                Self::write_block(file, bm, &block, compressed).await?;
            }
        }

        if !*inline {
            let (file, _) = spool.as_mut().expect("spool mode has a file");
            if !pending.is_empty() {
                Self::write_block(file, bm, &pending, compressed).await?;
            }
        }
        Ok(())
    }

    async fn open_spool(
        source_dir: &Path,
        new_source_id: &str,
    ) -> Result<(tokio::fs::File, PathBuf), BoxError> {
        fs::create_dir_all(source_dir).await?;
        let tmp_path = source_dir.join(format!("{}.tmp-{}", new_source_id, Uuid::new_v4()));
        let file = fs::File::create(&tmp_path).await?;
        Ok((file, tmp_path))
    }

    async fn cleanup_spool(
        spool: Option<(tokio::fs::File, PathBuf)>,
        _source_dir: &Path,
        _new_source_id: &str,
    ) {
        if let Some((file, tmp_path)) = spool {
            drop(file);
            let _ = fs::remove_file(&tmp_path).await;
        }
    }

    /// Write one (up to block-size) chunk: raw for plain storage, or compressed
    /// with the block header when `compressed` is set.
    async fn write_block(
        file: &mut tokio::fs::File,
        bm: &Arc<BlockManager>,
        block: &[u8],
        compressed: bool,
    ) -> Result<(), BoxError> {
        if !compressed {
            file.write_all(block).await?;
            return Ok(());
        }
        let bm = Arc::clone(bm);
        let block_owned = block.to_vec();
        let encoded = task::spawn_blocking(move || bm.compress_block(&block_owned))
            .await
            .map_err(|e| boxed_io_error(io::ErrorKind::Other, format!("compress task join error: {}", e)))??;
        file.write_all(&encoded).await?;
        Ok(())
    }

    pub async fn put_binary_data(
        &self,
        file_name: &str,
        input: &Bytes,
        _cover: bool,
        compressed: bool,
    ) -> Result<(), BoxError> {
        if file_name.is_empty() {
            return Err(boxed_io_error(io::ErrorKind::Other, "No filename provided"));
        }

        let new_size = input.len() as u64;

        // Hash + compression are CPU-bound; run off the runtime.
        let bm = Arc::clone(&self.bm);
        let input_for_blocking = input.clone();
        let (new_hash256, new_storage_bytes) = task::spawn_blocking(move || -> Result<(String, Vec<u8>), BoxError> {
            let hash = utils::get_hash256_from_binary(&input_for_blocking);
            let encoded = if compressed {
                bm.compress_all(&input_for_blocking)?
            } else {
                input_for_blocking.to_vec()
            };
            Ok((hash, encoded))
        })
        .await
        .map_err(|e| boxed_io_error(io::ErrorKind::Other, format!("encode task join error: {}", e)))??;

        // Object file write — concurrent, no lock.
        let new_source_id = Self::file_name_gen();
        self.persist_source_bytes(&new_source_id, &new_storage_bytes).await?;

        // Metadata commit — serialized by the DB executor.
        let out = match self
            .db
            .put_meta(file_name, &new_source_id, &new_hash256, compressed, new_size)
            .await
        {
            Ok(o) => o,
            Err(e) => {
                let _ = self.remove_source_file_if_exists(&new_source_id).await;
                return Err(e);
            }
        };

        if !out.keep_new {
            let _ = self.remove_source_file_if_exists(&new_source_id).await;
        }
        for source_id in &out.released {
            let _ = self.remove_source_file_if_exists(source_id).await;
        }

        Ok(())
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
            // Skip the redundant fs::exists check — fs::read returns NotFound
            // naturally if the file is missing, avoiding a TOCTOU window.
            let input = match fs::read(file_path).await {
                Ok(bytes) => Bytes::from(bytes),
                Err(err) if err.kind() == io::ErrorKind::NotFound => {
                    return Err(Box::new(io::Error::new(
                        io::ErrorKind::NotFound,
                        format!("File {} not found", &file),
                    )));
                }
                Err(err) => return Err(Box::new(err)),
            };
            self.put_binary_data(file_name, &input, cover, compressed).await?;
        }
        Ok(())
    }

    pub async fn delete(&self, pattern: &str, use_regx: bool) -> Result<(), BoxError> {
        if pattern == "" {
            return Err(boxed_io_error(io::ErrorKind::Other, "No files requested"));
        }

        let released = self.db.delete_meta(pattern, use_regx).await?;

        for source_id in &released {
            let _ = self.remove_source_file_if_exists(source_id).await;
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

    async fn persist_source_bytes(&self, source_id: &str, bytes: &[u8]) -> Result<(), BoxError> {
        let source_dir = self.source_dir(source_id);
        fs::create_dir_all(&source_dir).await?;

        let target_path = source_dir.join(source_id);
        let tmp_path = source_dir.join(format!("{}.tmp-{}", source_id, Uuid::new_v4()));

        let mut f = fs::File::create(&tmp_path).await?;
        f.write_all(bytes).await?;
        f.sync_all().await?;
        drop(f);

        if let Err(err) = fs::rename(&tmp_path, &target_path).await {
            let _ = fs::remove_file(&tmp_path).await;
            return Err(Box::new(err));
        }

        Ok(())
    }

    async fn remove_source_file_if_exists(&self, source_id: &str) -> Result<(), BoxError> {
        let source_path = self.source_path(source_id);
        match fs::remove_file(source_path).await {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(Box::new(err)),
        }
    }

    /// Clean orphan/tmp/tombstone files against the DB's known source ids.
    pub async fn reconcile_orphans(&self) -> Result<(), BoxError> {
        let known_ids: HashSet<String> = self.db.list_source_ids().await?.into_iter().collect();
        reconcile_orphans_files(&known_ids, &self.root).await
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
            if let Err(e) = self.file_info_collector(&path) {
                eprintln!("[linastore] tidy: skipping {}: {}", path.display(), e);
                continue;
            }
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

                    match stdfs::remove_file(&file_info.0) {
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

    fn file_info_collector(&mut self, path: &Path) -> Result<(), BoxError> {
        let hash_code = utils::get_hash256_from_file(path).map_err(|e| {
            boxed_io_error(
                io::ErrorKind::Other,
                format!("Hash of file {} generate error: {}", path.display(), e),
            )
        })?;

        let created_date = stdfs::metadata(path)
            .and_then(|metadata| metadata.created())
            .map_err(|e| {
                boxed_io_error(
                    io::ErrorKind::Other,
                    format!("Get file {} metadata/date error: {}", path.display(), e),
                )
            })?;

        let formated_created_date = DateTime::<Utc>::from(created_date)
            .format("%Y%m%d%H%M%S")
            .to_string();

        self.map_cache
            .entry(hash_code)
            .or_insert_with(Vec::new)
            .push((path.to_path_buf(), formated_created_date));

        Ok(())
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

    #[tokio::test]
    async fn test_reconcile_orphans_cleans_garbage_and_keeps_known_sources() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let sm = StoreManager::new(temp_dir.path()).await.expect("Failed to create StoreManager");
        let data = Bytes::from(vec![10u8, 20, 30, 40]);

        // Establish a real source via the public API.
        sm.put_binary_data("kept.bin", &data, false, false).await.expect("put");
        let links = sm
            .dao
            .get_links_by_name("kept.bin", false)
            .await
            .expect("links");
        let kept_id = links[0].source_id.clone();
        let kept_dir = sm.source_dir(&kept_id);

        // Plant garbage neighbors next to the real file.
        let orphan_id = "20991231235959aaaaaaaa";
        let orphan_path = kept_dir.join(orphan_id);
        let tombstone_path = kept_dir.join(format!("{}.deleting", orphan_id));
        let tmp_path = kept_dir.join(format!("{}.tmp-deadbeef", kept_id));
        stdfs::write(&orphan_path, b"orphan").unwrap();
        stdfs::write(&tombstone_path, b"tombstone").unwrap();
        stdfs::write(&tmp_path, b"tmp").unwrap();

        // Plant a tombstone for the KEPT source — reconciliation should resurrect it.
        let kept_path = sm.source_path(&kept_id);
        let kept_tombstone = kept_dir.join(format!("{}.deleting", kept_id));
        stdfs::rename(&kept_path, &kept_tombstone).unwrap();

        sm.reconcile_orphans().await.expect("reconcile");

        assert!(!orphan_path.exists(), "orphan source file should be removed");
        assert!(!tombstone_path.exists(), "orphan tombstone should be removed");
        assert!(!tmp_path.exists(), "tmp file should be removed");
        assert!(kept_path.exists(), "tombstone for known source should be restored");
        assert!(!kept_tombstone.exists(), "restored tombstone should be gone");

        let roundtrip = sm.get_binary_data("kept.bin").await.expect("get");
        assert_eq!(roundtrip, data);
    }

    async fn read_all(sm: &StoreManager, name: &str) -> Result<Vec<u8>, BoxError> {
        let mut reader = sm.open_read(name).await?.expect("reader");
        let mut out = Vec::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = reader.read_chunk(&mut buf).await?;
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
        }
        Ok(out)
    }

    #[tokio::test]
    async fn test_open_read_plain_and_compressed() {
        let temp_dir = TempDir::new().expect("tempdir");
        let sm = StoreManager::new(temp_dir.path()).await.expect("store");
        let data = generate_random_binary(4 * 1024 * 1024);

        for compressed in [false, true] {
            let name = if compressed { "stream.c.bin" } else { "stream.bin" };
            sm.put_binary_data(name, &data, false, compressed).await.expect("put");
            let got = read_all(&sm, name).await.expect("read");
            assert_eq!(got, data, "streamed read mismatch (compressed={})", compressed);
        }
    }

    #[tokio::test]
    async fn test_open_read_rejects_corruption() {
        let temp_dir = TempDir::new().expect("tempdir");
        let sm = StoreManager::new(temp_dir.path()).await.expect("store");
        let data = generate_random_binary(256 * 1024);
        sm.put_binary_data("corrupt.bin", &data, false, false).await.expect("put");

        let links = sm.dao.get_links_by_name("corrupt.bin", false).await.expect("links");
        let source_id = links[0].source_id.clone();
        let path = sm.source_path(&source_id);
        stdfs::write(&path, vec![0u8; data.len()]).expect("corrupt");

        let err = read_all(&sm, "corrupt.bin").await.expect_err("should fail integrity check");
        assert!(err.to_string().contains("integrity"), "err: {}", err);
    }

    #[tokio::test]
    async fn test_put_stream_roundtrip_plain_and_compressed() {
        let temp_dir = TempDir::new().expect("tempdir");
        let sm = StoreManager::new(temp_dir.path()).await.expect("store");
        let data = generate_random_binary(3 * 1024 * 1024);

        for compressed in [false, true] {
            let name = if compressed { "stream.c.bin" } else { "stream.bin" };
            let (tx, rx) = mpsc::channel(4);
            let data_clone = data.clone();
            let sender = tokio::spawn(async move {
                for c in data_clone.chunks(32 * 1024) {
                    tx.send(Bytes::copy_from_slice(c)).await.expect("send chunk");
                }
            });
            sm.put_stream(name, rx, Duration::from_secs(5), compressed, Some(data.len() as u64), None)
                .await
                .expect("put_stream");
            sender.await.expect("sender");
            let got = read_all(&sm, name).await.expect("read");
            assert_eq!(got, data, "streamed put mismatch (compressed={})", compressed);
        }
    }

    #[tokio::test]
    async fn test_put_stream_rejects_truncated() {
        let temp_dir = TempDir::new().expect("tempdir");
        let sm = StoreManager::new(temp_dir.path()).await.expect("store");

        let (tx, rx) = mpsc::channel(4);
        tx.send(Bytes::from(vec![1, 2, 3])).await.expect("send");
        drop(tx);

        let err = sm
            .put_stream("trunc.bin", rx, Duration::from_secs(5), false, Some(100), None)
            .await
            .expect_err("should reject truncated stream");
        assert!(err.to_string().contains("size mismatch"), "err: {}", err);
    }

    #[tokio::test]
    async fn test_put_stream_spools_large_and_dedups() {
        let temp_dir = TempDir::new().expect("tempdir");
        let sm = StoreManager::new(temp_dir.path()).await.expect("store");

        // Larger than INLINE_MEMORY_THRESHOLD: exercises the disk-spool path.
        let big = generate_random_binary(8 * 1024 * 1024);
        let (tx, rx) = mpsc::channel(8);
        let data_clone = big.clone();
        let sender = tokio::spawn(async move {
            for c in data_clone.chunks(128 * 1024) {
                tx.send(Bytes::copy_from_slice(c)).await.expect("send");
            }
        });
        if let Err(e) = sm.put_stream("big.bin", rx, Duration::from_secs(5), false, Some(big.len() as u64), None).await {
            panic!("put big failed: {}", e);
        }
        sender.await.expect("sender");
        assert_eq!(read_all(&sm, "big.bin").await.expect("read"), big);

        // Same content under a new name: must merge into the same source.
        let (tx, rx) = mpsc::channel(8);
        let data_clone = big.clone();
        let sender = tokio::spawn(async move {
            for c in data_clone.chunks(128 * 1024) {
                tx.send(Bytes::copy_from_slice(c)).await.expect("send");
            }
        });
        sm.put_stream("big2.bin", rx, Duration::from_secs(5), false, Some(big.len() as u64), None)
            .await
            .expect("put big2");
        sender.await.expect("sender");

        let links = sm.dao.get_links_by_name("big.bin", false).await.expect("links big");
        let links2 = sm.dao.get_links_by_name("big2.bin", false).await.expect("links big2");
        assert_eq!(links[0].source_id, links2[0].source_id, "dedup should share a source");
        let source = sm
            .dao
            .get_source_by_id(&links[0].source_id)
            .await
            .expect("get source")
            .expect("source exists");
        assert_eq!(source.count, 2, "refcount should reflect both links");

        assert_eq!(read_all(&sm, "big2.bin").await.expect("read big2"), big);
        assert_eq!(count_blob_files(temp_dir.path()), 1, "only one deduped blob on disk");
    }

    fn count_blob_files(root: &Path) -> usize {
        let mut count = 0;
        if let Ok(entries) = stdfs::read_dir(&root.join("linadata")) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if name == "logs" {
                        continue;
                    }
                    count += count_dir_blobs(&path);
                } else {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    if !name.ends_with(".db")
                        && !name.ends_with(".db-wal")
                        && !name.ends_with(".db-shm")
                    {
                        count += 1;
                    }
                }
            }
        }
        count
    }

    fn count_dir_blobs(dir: &Path) -> usize {
        let mut count = 0;
        if let Ok(entries) = stdfs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    count += count_dir_blobs(&path);
                } else {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    if !name.ends_with(".db")
                        && !name.ends_with(".db-wal")
                        && !name.ends_with(".db-shm")
                    {
                        count += 1;
                    }
                }
            }
        }
        count
    }

    #[test]
    fn test_relative_path_with_same_root() {
        let tm = TidyManager::new();

        let result = tm.relative_path_with_same_root("/a/b/c.txt", "/a/b/d.txt");
        assert_eq!(result, PathBuf::from("./d.txt"));

        let result = tm.relative_path_with_same_root("/a/b/c.txt", "/a/d.txt");
        assert_eq!(result, PathBuf::from("../d.txt"));
    }

    #[test]
    fn test_tidy_no_duplicates() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().to_path_buf();

        stdfs::write(dir.join("a.txt"), b"alpha").unwrap();
        stdfs::write(dir.join("b.txt"), b"beta").unwrap();

        let mut tm = TidyManager::new();
        tm.tidy(&dir, false).unwrap();

        assert!(dir.join("a.txt").exists());
        assert!(dir.join("b.txt").exists());
        assert!(!dir.join("a.txt").symlink_metadata().unwrap().file_type().is_symlink());
        assert!(!dir.join("b.txt").symlink_metadata().unwrap().file_type().is_symlink());
    }

    #[test]
    fn test_tidy_with_duplicates_creates_symlinks() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().to_path_buf();

        stdfs::write(dir.join("a.txt"), b"same content").unwrap();
        std::thread::sleep(std::time::Duration::from_secs(1));
        stdfs::write(dir.join("b.txt"), b"same content").unwrap();
        stdfs::write(dir.join("c.txt"), b"different").unwrap();

        let mut tm = TidyManager::new();
        tm.tidy(&dir, false).unwrap();

        let a_is_symlink = dir.join("a.txt").symlink_metadata().unwrap().file_type().is_symlink();
        let b_is_symlink = dir.join("b.txt").symlink_metadata().unwrap().file_type().is_symlink();
        let c_is_symlink = dir.join("c.txt").symlink_metadata().unwrap().file_type().is_symlink();

        assert!(!c_is_symlink, "unique file should not be a symlink");
        assert!(!a_is_symlink, "older duplicate should be kept as original");
        assert!(b_is_symlink, "newer duplicate should become symlink");
        assert_eq!(stdfs::read(dir.join("a.txt")).unwrap(), b"same content");
        assert_eq!(stdfs::read(dir.join("b.txt")).unwrap(), b"same content");
    }

    #[test]
    fn test_tidy_keep_newer() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().to_path_buf();

        stdfs::write(dir.join("old.txt"), b"same").unwrap();
        std::thread::sleep(std::time::Duration::from_secs(1));
        stdfs::write(dir.join("new.txt"), b"same").unwrap();

        let mut tm = TidyManager::new();
        tm.tidy(&dir, true).unwrap();

        assert!(!dir.join("new.txt").symlink_metadata().unwrap().file_type().is_symlink(),
                "newer file should be kept as original");
        assert!(dir.join("old.txt").symlink_metadata().unwrap().file_type().is_symlink(),
                "older file should become symlink");
    }

    #[test]
    fn test_tidy_empty_directory() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().to_path_buf();

        let mut tm = TidyManager::new();
        tm.tidy(&dir, false).unwrap();

        assert!(dir.read_dir().unwrap().next().is_none(), "empty dir should remain empty");
    }

    #[test]
    fn test_tidy_multiple_duplicate_groups() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().to_path_buf();

        stdfs::write(dir.join("g1_a.txt"), b"group1").unwrap();
        std::thread::sleep(std::time::Duration::from_secs(1));
        stdfs::write(dir.join("g1_b.txt"), b"group1").unwrap();
        stdfs::write(dir.join("g2_a.txt"), b"group2").unwrap();
        std::thread::sleep(std::time::Duration::from_secs(1));
        stdfs::write(dir.join("g2_b.txt"), b"group2").unwrap();
        stdfs::write(dir.join("g2_c.txt"), b"group2").unwrap();

        let mut tm = TidyManager::new();
        tm.tidy(&dir, false).unwrap();

        let g1_a_sym = dir.join("g1_a.txt").symlink_metadata().unwrap().file_type().is_symlink();
        let g1_b_sym = dir.join("g1_b.txt").symlink_metadata().unwrap().file_type().is_symlink();
        assert_ne!(g1_a_sym, g1_b_sym, "group1: exactly one should be symlink");

        let sym_count = ["g2_a.txt", "g2_b.txt", "g2_c.txt"]
            .iter()
            .filter(|f| dir.join(f).symlink_metadata().unwrap().file_type().is_symlink())
            .count();
        assert_eq!(sym_count, 2, "group2: two of three should be symlinks");

        assert_eq!(stdfs::read(dir.join("g1_a.txt")).unwrap(), b"group1");
        assert_eq!(stdfs::read(dir.join("g2_a.txt")).unwrap(), b"group2");
    }

    #[test]
    fn test_file_info_collector() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().to_path_buf();

        stdfs::write(dir.join("test.txt"), b"hello").unwrap();

        let mut tm = TidyManager::new();
        tm.file_info_collector(&dir.join("test.txt")).unwrap();

        assert_eq!(tm.map_cache.len(), 1);
        let hash = utils::get_hash256_from_file(&dir.join("test.txt")).unwrap();
        let entries = tm.map_cache.get(&hash).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, dir.join("test.txt"));
    }

    #[test]
    fn test_find_extreme_file() {
        let tm = TidyManager::new();
        let files = vec![
            (PathBuf::from("/a/old.txt"), "20200101000000".to_string()),
            (PathBuf::from("/a/mid.txt"), "20220101000000".to_string()),
            (PathBuf::from("/a/new.txt"), "20240101000000".to_string()),
        ];

        let oldest = tm.find_extreme_file(&files, |a, b| a < b);
        assert_eq!(oldest.0, &PathBuf::from("/a/old.txt"));

        let newest = tm.find_extreme_file(&files, |a, b| a > b);
        assert_eq!(newest.0, &PathBuf::from("/a/new.txt"));
    }
}
