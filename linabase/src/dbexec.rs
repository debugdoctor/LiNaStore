//! Single-threaded metadata executor. SQLite is single-writer, so all metadata
//! access funnels through one actor owning the sole `Dao`; commands run serially,
//! making multi-step ops (dedup, refcounts, link updates) atomic. Object file IO
//! stays out of here so workers can do it concurrently.

use std::collections::HashSet;
use std::error::Error;
use std::io;
use std::path::{Path, PathBuf};

use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::dao::{Dao, DirEntry, Link};

pub type BoxError = Box<dyn Error + Send + Sync>;

pub struct GetMetaOut {
    pub source_id: String,
    pub compressed: bool,
    pub size: u64,
    pub hash256: String,
}

pub struct PutMetaOut {
    /// Keep the worker's persisted file; `false` when dedup reused a source.
    pub keep_new: bool,
    /// Source ids now unreferenced; the worker must delete their files.
    pub released: Vec<String>,
}

pub(crate) enum DbCmd {
    GetMeta {
        file_name: String,
        reply: oneshot::Sender<Result<Option<GetMetaOut>, BoxError>>,
    },
    PutMeta {
        file_name: String,
        new_source_id: String,
        hash256: String,
        compressed: bool,
        size: u64,
        reply: oneshot::Sender<Result<PutMetaOut, BoxError>>,
    },
    /// Pre-write dedup check: if no link exists for `file_name` and a source
    /// with this hash (and compression flag) already exists, add a link to it
    /// and return its id — letting the caller skip the file write entirely.
    DedupOrLink {
        file_name: String,
        hash256: String,
        compressed: bool,
        reply: oneshot::Sender<Result<Option<String>, BoxError>>,
    },
    DeleteMeta {
        pattern: String,
        use_regex: bool,
        reply: oneshot::Sender<Result<Vec<String>, BoxError>>,
    },
    List {
        pattern: String,
        n: u64,
        isext: bool,
        use_regex: bool,
        reply: oneshot::Sender<Result<Vec<Link>, BoxError>>,
    },
    ListSourceIds {
        reply: oneshot::Sender<Result<Vec<String>, BoxError>>,
    },
    IsDir {
        path: String,
        reply: oneshot::Sender<Result<bool, BoxError>>,
    },
    ListChildDirs {
        parent: String,
        reply: oneshot::Sender<Result<Vec<DirEntry>, BoxError>>,
    },
    AllDirs {
        reply: oneshot::Sender<Result<Vec<DirEntry>, BoxError>>,
    },
    Mkdir {
        path: String,
        parent: String,
        reply: oneshot::Sender<Result<(), BoxError>>,
    },
    Rmdir {
        path: String,
        reply: oneshot::Sender<Result<(), BoxError>>,
    },
    SetFileMode {
        name: String,
        mode: u32,
        reply: oneshot::Sender<Result<(), BoxError>>,
    },
    SetDirMode {
        path: String,
        mode: u32,
        reply: oneshot::Sender<Result<(), BoxError>>,
    },
}

/// Client handle kept by `StoreManager`. Cloning shares the same queue.
#[derive(Clone)]
pub struct DbClient {
    tx: mpsc::Sender<DbCmd>,
}

impl std::fmt::Debug for DbClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DbClient")
    }
}

impl DbClient {
    pub(crate) fn new(tx: mpsc::Sender<DbCmd>) -> Self {
        Self { tx }
    }

    async fn call<T, F>(&self, make: F) -> Result<T, BoxError>
    where
        F: FnOnce(oneshot::Sender<Result<T, BoxError>>) -> DbCmd,
    {
        let (rtx, rrx) = oneshot::channel();
        self.tx
            .send(make(rtx))
            .await
            .map_err(|_| boxed_io_error(io::ErrorKind::Other, "DB executor not running"))?;
        rrx
            .await
            .map_err(|_| boxed_io_error(io::ErrorKind::Other, "DB executor closed"))?
    }

    pub async fn get_meta(&self, file_name: &str) -> Result<Option<GetMetaOut>, BoxError> {
        let n = file_name.to_string();
        self.call(move |reply| DbCmd::GetMeta { file_name: n, reply })
            .await
    }

    pub async fn put_meta(
        &self,
        file_name: &str,
        new_source_id: &str,
        hash256: &str,
        compressed: bool,
        size: u64,
    ) -> Result<PutMetaOut, BoxError> {
        let (n, id, h) = (
            file_name.to_string(),
            new_source_id.to_string(),
            hash256.to_string(),
        );
        self.call(move |reply| DbCmd::PutMeta {
            file_name: n,
            new_source_id: id,
            hash256: h,
            compressed,
            size,
            reply,
        })
        .await
    }

    pub async fn delete_meta(&self, pattern: &str, use_regex: bool) -> Result<Vec<String>, BoxError> {
        let p = pattern.to_string();
        self.call(move |reply| DbCmd::DeleteMeta {
            pattern: p,
            use_regex,
            reply,
        })
        .await
    }

    pub async fn dedup_or_link(
        &self,
        file_name: &str,
        hash256: &str,
        compressed: bool,
    ) -> Result<Option<String>, BoxError> {
        let (n, h) = (file_name.to_string(), hash256.to_string());
        self.call(move |reply| DbCmd::DedupOrLink {
            file_name: n,
            hash256: h,
            compressed,
            reply,
        })
        .await
    }

    pub async fn list(
        &self,
        pattern: &str,
        n: u64,
        isext: bool,
        use_regex: bool,
    ) -> Result<Vec<Link>, BoxError> {
        let p = pattern.to_string();
        self.call(move |reply| DbCmd::List {
            pattern: p,
            n,
            isext,
            use_regex,
            reply,
        })
        .await
    }

    pub async fn list_source_ids(&self) -> Result<Vec<String>, BoxError> {
        self.call(|reply| DbCmd::ListSourceIds { reply }).await
    }

    pub async fn is_dir(&self, path: &str) -> Result<bool, BoxError> {
        let p = path.to_string();
        self.call(move |reply| DbCmd::IsDir { path: p, reply })
            .await
    }

    pub async fn list_child_dirs(&self, parent: &str) -> Result<Vec<DirEntry>, BoxError> {
        let p = parent.to_string();
        self.call(move |reply| DbCmd::ListChildDirs { parent: p, reply })
            .await
    }

    pub async fn all_dirs(&self) -> Result<Vec<DirEntry>, BoxError> {
        self.call(|reply| DbCmd::AllDirs { reply }).await
    }

    pub async fn mkdir(&self, path: &str, parent: &str) -> Result<(), BoxError> {
        let (p, pa) = (path.to_string(), parent.to_string());
        self.call(move |reply| DbCmd::Mkdir {
            path: p,
            parent: pa,
            reply,
        })
        .await
    }

    pub async fn rmdir(&self, path: &str) -> Result<(), BoxError> {
        let p = path.to_string();
        self.call(move |reply| DbCmd::Rmdir { path: p, reply })
            .await
    }

    pub async fn set_file_mode(&self, name: &str, mode: u32) -> Result<(), BoxError> {
        let n = name.to_string();
        self.call(move |reply| DbCmd::SetFileMode {
            name: n,
            mode,
            reply,
        })
        .await
    }

    pub async fn set_dir_mode(&self, path: &str, mode: u32) -> Result<(), BoxError> {
        let p = path.to_string();
        self.call(move |reply| DbCmd::SetDirMode {
            path: p,
            mode,
            reply,
        })
        .await
    }
}

pub struct DbExecutor;

impl DbExecutor {
    /// Owns the `Dao`, runs startup reconciliation, then serves commands until
    /// the queue is dropped.
    pub(crate) async fn spawn(
        root: PathBuf,
        rx: mpsc::Receiver<DbCmd>,
        ready: oneshot::Sender<Result<(), BoxError>>,
    ) {
        let result = async {
            tokio::fs::create_dir_all(root.join("linadata"))
                .await
                .map_err(|e| boxed_io_error(e.kind(), format!("create linadata: {}", e)))?;
            let dao = Dao::new(root.join("linadata").join("meta.db"))
                .await
                .map_err(dao_to_io_error)?;
            reconcile_startup(&dao, &root).await?;
            Ok::<_, BoxError>((dao, root))
        }
        .await;

        let (dao, root) = match result {
            Ok(v) => {
                let _ = ready.send(Ok(()));
                v
            }
            Err(e) => {
                let _ = ready.send(Err(e));
                return;
            }
        };

        Self::run(dao, root, rx).await;
    }

    async fn run(dao: Dao, root: PathBuf, mut rx: mpsc::Receiver<DbCmd>) {
        let _ = &root;
        while let Some(cmd) = rx.recv().await {
            handle(&dao, cmd).await;
        }
    }
}

async fn reconcile_startup(dao: &Dao, root: &Path) -> Result<(), BoxError> {
    let known_ids: HashSet<String> = dao
        .list_source_ids()
        .await
        .map_err(dao_to_io_error)?
        .into_iter()
        .collect();
    reconcile_orphans_files(&known_ids, root).await?;
    sync_dirs_from_links(dao).await
}

async fn handle(dao: &Dao, cmd: DbCmd) {
    match cmd {
        DbCmd::GetMeta { file_name, reply } => {
            let _ = reply.send(get_meta(dao, &file_name).await);
        }
        DbCmd::PutMeta {
            file_name,
            new_source_id,
            hash256,
            compressed,
            size,
            reply,
        } => {
            let _ = reply
                .send(put_meta(dao, &file_name, &new_source_id, &hash256, compressed, size).await);
        }
        DbCmd::DedupOrLink {
            file_name,
            hash256,
            compressed,
            reply,
        } => {
            let _ = reply
                .send(dedup_or_link(dao, &file_name, &hash256, compressed).await);
        }
        DbCmd::DeleteMeta {
            pattern,
            use_regex,
            reply,
        } => {
            let _ = reply.send(delete_meta(dao, &pattern, use_regex).await);
        }
        DbCmd::List {
            pattern,
            n,
            isext,
            use_regex,
            reply,
        } => {
            let _ = reply.send(list_locked(dao, &pattern, n, isext, use_regex).await);
        }
        DbCmd::ListSourceIds { reply } => {
            let _ = reply.send(dao.list_source_ids().await.map_err(dao_to_boxed_error));
        }
        DbCmd::IsDir { path, reply } => {
            let r = dao
                .get_dir_by_path(&path)
                .await
                .map_err(dao_to_boxed_error)
                .map(|d| d.is_some());
            let _ = reply.send(r);
        }
        DbCmd::ListChildDirs { parent, reply } => {
            let _ = reply.send(dao.list_dirs_by_parent(&parent).await.map_err(dao_to_boxed_error));
        }
        DbCmd::AllDirs { reply } => {
            let _ = reply.send(dao.list_all_dirs().await.map_err(dao_to_boxed_error));
        }
        DbCmd::Mkdir { path, parent, reply } => {
            let _ = reply.send(dao.insert_dir(&path, &parent).await.map_err(dao_to_boxed_error));
        }
        DbCmd::Rmdir { path, reply } => {
            let _ = reply.send(dao.delete_dir(&path).await.map_err(dao_to_boxed_error));
        }
        DbCmd::SetFileMode { name, mode, reply } => {
            let _ = reply.send(dao.set_link_mode(&name, mode).await.map_err(dao_to_boxed_error));
        }
        DbCmd::SetDirMode { path, mode, reply } => {
            let _ = reply.send(dao.set_dir_mode(&path, mode).await.map_err(dao_to_boxed_error));
        }
    }
}

async fn get_meta(dao: &Dao, file_name: &str) -> Result<Option<GetMetaOut>, BoxError> {
    let links = dao
        .get_links_by_name(file_name, false)
        .await
        .map_err(dao_to_io_error)?;
    let link = match links.first() {
        Some(l) => l,
        None => return Ok(None),
    };
    let source = dao
        .get_source_by_id(&link.source_id)
        .await
        .map_err(dao_to_io_error)?
        .ok_or_else(|| boxed_io_error(io::ErrorKind::NotFound, "File not found"))?;
    Ok(Some(GetMetaOut {
        source_id: source.id,
        compressed: source.compressed,
        size: source.size,
        hash256: source.hash256,
    }))
}

/// Apply a put to metadata. Runs serially on the executor; the caller already
/// persisted the object file, so this decides keep-vs-dedup and returns sources
/// that became unreferenced.
async fn put_meta(
    dao: &Dao,
    file_name: &str,
    new_source_id: &str,
    hash256: &str,
    compressed: bool,
    size: u64,
) -> Result<PutMetaOut, BoxError> {
    let ext = Path::new(file_name)
        .extension()
        .unwrap_or_default()
        .to_str()
        .unwrap_or("")
        .to_string();

    let mut out = PutMetaOut {
        keep_new: true,
        released: Vec::new(),
    };

    let links = dao
        .get_links_by_name(file_name, false)
        .await
        .map_err(dao_to_io_error)?;

    if let Some(link) = links.first() {
        let source = dao
            .get_source_by_id(&link.source_id)
            .await
            .map_err(dao_to_io_error)?
            .ok_or_else(|| boxed_io_error(io::ErrorKind::NotFound, "Source not found"))?;

        // Identical content already stored: nothing to replace.
        if source.hash256 == hash256 && source.compressed == compressed {
            out.keep_new = false;
            return Ok(out);
        }

        // New version: reference the new file, repoint the link, release the
        // old source if its refcount hits zero.
        dao.insert_source(new_source_id, hash256, compressed, size)
            .await
            .map_err(dao_to_io_error)?;
        dao.update_link_source_id(&link.id, new_source_id)
            .await
            .map_err(dao_to_io_error)?;

        let new_count = source
            .count
            .checked_sub(1)
            .ok_or_else(|| boxed_io_error(io::ErrorKind::Other, "Source count is 0"))?;
        if new_count > 0 {
            dao.update_source(
                &source.id,
                &source.hash256,
                source.compressed,
                source.size,
                new_count,
            )
            .await
            .map_err(dao_to_io_error)?;
        } else {
            dao.delete_source_by_id(&source.id)
                .await
                .map_err(dao_to_io_error)?;
            out.released.push(source.id.clone());
        }
        return Ok(out);
    }

    // New file: dedup against existing sources by content hash.
    if let Some(existing) = dao
        .get_source_by_hash256(hash256)
        .await
        .map_err(dao_to_io_error)?
    {
        if existing.compressed == compressed {
            let link_id = Uuid::new_v4().to_string();
            dao.insert_link_with_id(&link_id, file_name, &ext, &existing.id, 420)
                .await
                .map_err(dao_to_io_error)?;
            dao.update_source(
                &existing.id,
                &existing.hash256,
                existing.compressed,
                existing.size,
                existing.count + 1,
            )
            .await
            .map_err(dao_to_io_error)?;
            out.keep_new = false; // reuse existing source; worker deletes the new file
            return Ok(out);
        }
    }

    let link_id = Uuid::new_v4().to_string();
    dao.insert_source(new_source_id, hash256, compressed, size)
        .await
        .map_err(dao_to_io_error)?;
    dao.insert_link_with_id(&link_id, file_name, &ext, new_source_id, 420)
        .await
        .map_err(dao_to_io_error)?;
    Ok(out)
}

/// Pre-write dedup: if no link exists yet and a source with this content hash
/// (and compression flag) is already stored, add a link to it and return its
/// id so the caller can skip writing a duplicate object file.
async fn dedup_or_link(
    dao: &Dao,
    file_name: &str,
    hash256: &str,
    compressed: bool,
) -> Result<Option<String>, BoxError> {
    let links = dao
        .get_links_by_name(file_name, false)
        .await
        .map_err(dao_to_io_error)?;
    if !links.is_empty() {
        return Ok(None); // put_meta will handle the versioning path
    }
    let existing = match dao
        .get_source_by_hash256(hash256)
        .await
        .map_err(dao_to_io_error)?
    {
        Some(src) if src.compressed == compressed => src,
        _ => return Ok(None),
    };
    let ext = Path::new(file_name)
        .extension()
        .unwrap_or_default()
        .to_str()
        .unwrap_or("")
        .to_string();
    let link_id = Uuid::new_v4().to_string();
    dao.insert_link_with_id(&link_id, file_name, &ext, &existing.id, 420)
        .await
        .map_err(dao_to_io_error)?;
    dao.update_source(
        &existing.id,
        &existing.hash256,
        existing.compressed,
        existing.size,
        existing.count + 1,
    )
    .await
    .map_err(dao_to_io_error)?;
    Ok(Some(existing.id))
}

/// Delete matching links, decrement refcounts, return unreferenced source ids.
async fn delete_meta(
    dao: &Dao,
    pattern: &str,
    use_regex: bool,
) -> Result<Vec<String>, BoxError> {
    let links = list_locked(dao, pattern, 0, false, use_regex).await?;
    let mut released = Vec::new();

    for link in links {
        let source = dao
            .get_source_by_id(&link.source_id)
            .await
            .map_err(dao_to_io_error)?
            .ok_or_else(|| boxed_io_error(io::ErrorKind::NotFound, "File not found"))?;
        dao.delete_link_by_id(&link.id)
            .await
            .map_err(dao_to_io_error)?;

        let new_count = source
            .count
            .checked_sub(1)
            .ok_or_else(|| boxed_io_error(io::ErrorKind::Other, "Source count is 0"))?;
        if new_count > 0 {
            dao.update_source(
                &source.id,
                &source.hash256,
                source.compressed,
                source.size,
                new_count,
            )
            .await
            .map_err(dao_to_io_error)?;
        } else {
            dao.delete_source_by_id(&source.id)
                .await
                .map_err(dao_to_io_error)?;
            released.push(source.id.clone());
        }
    }

    Ok(released)
}

async fn list_locked(
    dao: &Dao,
    pattern: &str,
    n: u64,
    isext: bool,
    use_regex: bool,
) -> Result<Vec<Link>, BoxError> {
    let links = if isext {
        dao.get_links_by_ext(pattern).await.map_err(dao_to_io_error)?
    } else if (pattern == "" || pattern == "*") && use_regex {
        dao.get_n_links(n).await.map_err(dao_to_io_error)?
    } else if pattern.contains('*') && use_regex {
        let sql_pattern = pattern.replace('*', "%");
        dao.get_links_by_name(&sql_pattern, true)
            .await
            .map_err(dao_to_io_error)?
    } else {
        dao.get_links_by_name(pattern, false)
            .await
            .map_err(dao_to_io_error)?
    };
    Ok(links)
}

async fn sync_dirs_from_links(dao: &Dao) -> Result<(), BoxError> {
    let links = list_locked(dao, "*", 0, false, true).await?;
    for link in &links {
        if let Some(slash) = link.name.rfind('/') {
            let parent = &link.name[..slash];
            let parts: Vec<&str> = parent.split('/').collect();
            let mut acc = String::new();
            let mut prev = String::new();
            for part in &parts {
                if !acc.is_empty() {
                    acc.push('/');
                }
                acc.push_str(part);
                let _ = dao.insert_dir(&acc, &prev).await;
                prev = acc.clone();
            }
        }
    }
    Ok(())
}

/// Filesystem cleanup against a set of known source ids. Pure file IO; used at
/// startup and by `StoreManager::reconcile_orphans`.
pub async fn reconcile_orphans_files(
    known_ids: &HashSet<String>,
    root: &Path,
) -> Result<(), BoxError> {
    let linadata_root = root.join("linadata");
    let mut removed_tmp = 0u64;
    let mut removed_tombstone = 0u64;
    let mut removed_orphan = 0u64;

    // The expected layout is linadata/<id[0..4]>/<id[4..6]>/<id>. Only
    // descend two levels so we don't accidentally chew on meta.db / logs.
    let mut top = match std::fs::read_dir(&linadata_root) {
        Ok(rd) => rd,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(Box::new(err)),
    };

    while let Some(top_entry) = top.next() {
        let top_entry = top_entry?;
        let top_path = top_entry.path();
        let top_meta = match top_entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if !top_meta.is_dir() {
            continue;
        }
        if top_path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.len() != 4)
            .unwrap_or(true)
        {
            continue;
        }

        let mid = match std::fs::read_dir(&top_path) {
            Ok(rd) => rd,
            Err(_) => continue,
        };
        for mid_entry in mid {
            let mid_entry = mid_entry?;
            let mid_path = mid_entry.path();
            let mid_meta = match mid_entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            if !mid_meta.is_dir() {
                continue;
            }
            if mid_path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.len() != 2)
                .unwrap_or(true)
            {
                continue;
            }

            let leaves = match std::fs::read_dir(&mid_path) {
                Ok(rd) => rd,
                Err(_) => continue,
            };
            for leaf in leaves {
                let leaf = leaf?;
                let leaf_path = leaf.path();
                let leaf_meta = match leaf.metadata() {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                if !leaf_meta.is_file() {
                    continue;
                }
                let name = match leaf_path.file_name().and_then(|n| n.to_str()) {
                    Some(s) => s.to_string(),
                    None => continue,
                };

                if name.contains(".tmp-") {
                    if tokio::fs::remove_file(&leaf_path).await.is_ok() {
                        removed_tmp += 1;
                    }
                    continue;
                }
                if let Some(stem) = name.strip_suffix(".deleting") {
                    if known_ids.contains(stem) {
                        let restored = mid_path.join(stem);
                        let _ = tokio::fs::rename(&leaf_path, &restored).await;
                    } else if tokio::fs::remove_file(&leaf_path).await.is_ok() {
                        removed_tombstone += 1;
                    }
                    continue;
                }

                if !known_ids.contains(&name) {
                    if tokio::fs::remove_file(&leaf_path).await.is_ok() {
                        removed_orphan += 1;
                    }
                }
            }
        }
    }

    if removed_tmp | removed_tombstone | removed_orphan > 0 {
        eprintln!(
            "[linastore] reconcile: removed_tmp={} removed_tombstone={} removed_orphan={}",
            removed_tmp, removed_tombstone, removed_orphan
        );
    }
    Ok(())
}

fn boxed_io_error(kind: io::ErrorKind, message: impl Into<String>) -> BoxError {
    Box::new(io::Error::new(kind, message.into()))
}

fn dao_to_io_error(err: anyhow::Error) -> io::Error {
    io::Error::other(err.to_string())
}

fn dao_to_boxed_error(err: anyhow::Error) -> BoxError {
    Box::new(io::Error::other(err.to_string()))
}
