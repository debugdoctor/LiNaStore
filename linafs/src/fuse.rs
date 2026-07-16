use std::{
    collections::{HashMap, HashSet},
    ffi::OsStr,
    hash::{Hash, Hasher},
    path::Path,
    sync::{Arc, RwLock},
    time::{Duration, UNIX_EPOCH},
};

use bytes::Bytes;
use fuser::{
    FileAttr, FileHandle, FileType, Filesystem, LockOwner, OpenFlags, ReplyAttr,
    ReplyCreate, ReplyData, ReplyDirectory, ReplyEmpty, ReplyEntry, ReplyLseek,
    ReplyOpen, ReplyStatfs, ReplyWrite, Request, TimeOrNow,
};
use linabase::{dao::Link, service::StoreManager};

fn file_ino(name: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut h);
    let ino = h.finish();
    if ino == 0 || ino == ROOT_INO {
        ino ^ 0xDEAD
    } else {
        ino
    }
}

fn dir_ino(dir_path: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    "d:".hash(&mut h);
    dir_path.hash(&mut h);
    let ino = h.finish();
    if ino == 0 || ino == ROOT_INO {
        ino ^ 0xDEAD
    } else {
        ino
    }
}

const ROOT_INO: u64 = 1;
const TTL: Duration = Duration::from_secs(1);

fn make_attr(ino: u64, size: u64, kind: FileType, perm: u16) -> FileAttr {
    FileAttr {
        ino: fuser::INodeNo(ino),
        size,
        blocks: (size + 511) / 512,
        kind,
        perm,
        nlink: if kind == FileType::Directory { 2 } else { 1 },
        uid: 0,
        gid: 0,
        rdev: 0,
        blksize: 4096,
        flags: 0,
        atime: UNIX_EPOCH,
        mtime: UNIX_EPOCH,
        ctime: UNIX_EPOCH,
        crtime: UNIX_EPOCH,
    }
}

const S_IFMT: u32 = 0o170000;
const S_IFREG: u32 = 0o100000;
const S_IFLNK: u32 = 0o120000;
const S_IFDIR: u32 = 0o040000;

const MODE_FILE: u32 = S_IFREG | 0o644;
const MODE_SYMLINK: u32 = S_IFLNK | 0o777;
const MODE_DIR: u32 = S_IFDIR | 0o755;

fn kind_from_mode(mode: u32) -> FileType {
    if mode >= S_IFREG {
        match mode & S_IFMT {
            S_IFLNK => FileType::Symlink,
            S_IFDIR => FileType::Directory,
            _ => FileType::RegularFile,
        }
    } else {
        FileType::RegularFile
    }
}

fn perm_from_mode(mode: u32) -> u16 {
    (mode & 0o777) as u16
}

fn direct_child_name(full: &str, prefix: &str) -> Option<String> {
    let stripped = if prefix.is_empty() {
        full
    } else if let Some(s) = full.strip_prefix(&format!("{}/", prefix)) {
        s
    } else {
        return None;
    };
    if stripped.contains('/') {
        None
    } else {
        Some(stripped.to_string())
    }
}

pub struct LinaFs {
    store: Arc<StoreManager>,
    rt: tokio::runtime::Handle,
    write_buf: RwLock<HashMap<u64, Vec<u8>>>,
    compressed: bool,
}

impl LinaFs {
    pub async fn new(root: &str, compressed: bool) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let store = StoreManager::new(root).await?;
        Ok(Self {
            store: Arc::new(store),
            rt: tokio::runtime::Handle::current(),
            write_buf: RwLock::new(HashMap::new()),
            compressed,
        })
    }

    fn rt<F, T>(&self, f: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        self.rt.block_on(f)
    }

    fn all_files(&self) -> Vec<Link> {
        self.rt(self.store.list("*", 0, false, true))
            .unwrap_or_default()
    }

    fn resolve_dir_path(&self, ino: u64) -> Option<String> {
        let dirs = self.rt(self.store.all_dirs()).unwrap_or_default();
        dirs.into_iter()
            .find(|d| dir_ino(&d.path) == ino)
            .map(|d| d.path)
    }
}

impl Filesystem for LinaFs {
    fn lookup(&self, _req: &Request, parent: fuser::INodeNo, name: &OsStr, reply: ReplyEntry) {
        let parent_val: u64 = parent.into();

        let parent_path = if parent_val == ROOT_INO {
            String::new()
        } else {
            match self.resolve_dir_path(parent_val) {
                Some(p) => p,
                None => return reply.error(fuser::Errno::ENOENT),
            }
        };

        let name_str = match name.to_str() {
            Some(n) => n,
            None => return reply.error(fuser::Errno::EINVAL),
        };

        let full_path = if parent_path.is_empty() {
            name_str.to_string()
        } else {
            format!("{}/{}", parent_path, name_str)
        };

        match self.rt(self.store.get_binary_data(&full_path)) {
            Ok(data) => {
                let ino = file_ino(&full_path);
                let link = self.all_files().iter().find(|l| l.name == full_path);
                let mode = link.map(|l| l.mode).unwrap_or(MODE_FILE);
                let attr = make_attr(ino, data.len() as u64, kind_from_mode(mode), perm_from_mode(mode));
                return reply.entry(&TTL, &attr, fuser::Generation(0));
            }
            Err(_) => {}
        }

        if self.rt(self.store.is_dir(&full_path)).unwrap_or(false) {
            let ino = dir_ino(&full_path);
            let attr = make_attr(ino, 0, FileType::Directory, 0o755);
            return reply.entry(&TTL, &attr, fuser::Generation(0));
        }

        reply.error(fuser::Errno::ENOENT);
    }

    fn getattr(
        &self,
        _req: &Request,
        ino: fuser::INodeNo,
        _fh: Option<FileHandle>,
        reply: ReplyAttr,
    ) {
        let ino_val: u64 = ino.into();

        if ino_val == ROOT_INO {
            let attr = make_attr(ROOT_INO, 0, FileType::Directory, 0o755);
            return reply.attr(&TTL, &attr);
        }

        for link in self.all_files().iter() {
            if file_ino(&link.name) == ino_val {
                match self.rt(self.store.get_binary_data(&link.name)) {
                    Ok(data) => {
                        let attr = make_attr(
                            ino_val,
                            data.len() as u64,
                            kind_from_mode(link.mode),
                            perm_from_mode(link.mode),
                        );
                        return reply.attr(&TTL, &attr);
                    }
                    Err(_) => return reply.error(fuser::Errno::EIO),
                }
            }
        }

        let dirs = self.rt(self.store.all_dirs()).unwrap_or_default();
        if let Some(d) = dirs.iter().find(|d| dir_ino(&d.path) == ino_val) {
            let attr = make_attr(ino_val, 0, FileType::Directory, d.mode as u16);
            return reply.attr(&TTL, &attr);
        }

        reply.error(fuser::Errno::ENOENT);
    }

    fn setattr(
        &self,
        _req: &Request,
        ino: fuser::INodeNo,
        mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<TimeOrNow>,
        _mtime: Option<TimeOrNow>,
        _ctime: Option<std::time::SystemTime>,
        _fh: Option<FileHandle>,
        _crtime: Option<std::time::SystemTime>,
        _chgtime: Option<std::time::SystemTime>,
        _bkuptime: Option<std::time::SystemTime>,
        _flags: Option<fuser::BsdFileFlags>,
        reply: ReplyAttr,
    ) {
        let ino_val: u64 = ino.into();

        // Root directory
        if ino_val == ROOT_INO {
            let perm = mode.unwrap_or(0o755) as u16;
            let attr = make_attr(ROOT_INO, 0, FileType::Directory, perm);
            return reply.attr(&TTL, &attr);
        }

        // Other directories
        let dirs = self.rt(self.store.all_dirs()).unwrap_or_default();
        if let Some(d) = dirs.iter().find(|d| dir_ino(&d.path) == ino_val) {
            if let Some(m) = mode {
                let _ = self.rt(self.store.set_dir_mode(&d.path, m));
            }
            let attr = make_attr(ino_val, 0, FileType::Directory, perm_from_mode(d.mode));
            return reply.attr(&TTL, &attr);
        }

        // File: resolve name
        let name = match self
            .all_files()
            .iter()
            .find(|l| file_ino(&l.name) == ino_val)
        {
            Some(l) => l.name.clone(),
            None => return reply.error(fuser::Errno::ENOENT),
        };

        // Handle truncation
        if let Some(new_size) = size {
            let truncated = {
                let mut buf = self.write_buf.write().unwrap();
                if let Some(data) = buf.get_mut(&ino_val) {
                    data.resize(new_size as usize, 0);
                    data.clone()
                } else {
                    match self.rt(self.store.get_binary_data(&name)) {
                        Ok(d) => {
                            let mut v = d.to_vec();
                            v.resize(new_size as usize, 0);
                            v
                        }
                        Err(_) => return reply.error(fuser::Errno::EIO),
                    }
                }
            };
            if self
                .rt(self
                    .store
                    .put_binary_data(&name, &Bytes::from(truncated), true, self.compressed))
                .is_err()
            {
                return reply.error(fuser::Errno::EIO);
            }
        }

        let link = self.all_files().iter().find(|l| file_ino(&l.name) == ino_val);
        let file_mode = link.map(|l| l.mode).unwrap_or(MODE_FILE);

        if let Some(m) = mode {
            let new_mode = (file_mode & S_IFMT) | (m & 0o777);
            let _ = self.rt(self.store.set_file_mode(&name, new_mode));
        }

        let data_len = {
            let buf = self.write_buf.read().unwrap();
            buf.get(&ino_val)
                .map(|d| d.len() as u64)
                .unwrap_or_else(|| {
                    self.rt(self.store.get_binary_data(&name))
                        .map(|d| d.len() as u64)
                        .unwrap_or(0)
                })
        };

        let attr = make_attr(ino_val, data_len, kind_from_mode(file_mode), perm_from_mode(file_mode));
        reply.attr(&TTL, &attr);
    }

    fn readdir(
        &self,
        _req: &Request,
        ino: fuser::INodeNo,
        _fh: FileHandle,
        offset: u64,
        mut reply: ReplyDirectory,
    ) {
        let ino_val: u64 = ino.into();

        let (prefix, parent_ino) = if ino_val == ROOT_INO {
            (String::new(), ROOT_INO)
        } else {
            match self.resolve_dir_path(ino_val) {
                Some(p) => (p, ROOT_INO),
                None => return reply.error(fuser::Errno::ENOENT),
            }
        };

        let mut entries: Vec<(u64, FileType, String)> = Vec::new();
        entries.push((ino_val, FileType::Directory, ".".into()));
        entries.push((parent_ino, FileType::Directory, "..".into()));

        // Add child directories from dir table
        let child_dirs = self.rt(self.store.list_child_dirs(&prefix)).unwrap_or_default();
        for d in &child_dirs {
            if let Some(leaf) = direct_child_name(&d.path, &prefix) {
                entries.push((dir_ino(&d.path), FileType::Directory, leaf));
            }
        }

        // Add child files from link table (direct children only)
        let files = self.all_files();
        let mut seen: HashSet<String> = child_dirs
            .iter()
            .filter_map(|d| direct_child_name(&d.path, &prefix))
            .collect();

        for link in &files {
            if let Some(leaf) = direct_child_name(&link.name, &prefix) {
                if !seen.contains(&leaf) {
                    let full = if prefix.is_empty() {
                        leaf.clone()
                    } else {
                        format!("{}/{}", prefix, leaf)
                    };
                    entries.push((file_ino(&full), kind_from_mode(link.mode), leaf));
                    seen.insert(leaf);
                }
            }
        }

        let entries_len = entries.len();
        for (i, (e_ino, kind, name)) in entries.iter().enumerate().skip(offset as usize) {
            if reply.add(fuser::INodeNo(*e_ino), (i + 1) as u64, *kind, name) {
                break;
            }
        }
        reply.ok();
    }

    fn open(
        &self,
        _req: &Request,
        _ino: fuser::INodeNo,
        _flags: OpenFlags,
        reply: ReplyOpen,
    ) {
        reply.opened(FileHandle(0), fuser::FopenFlags::empty());
    }

    fn read(
        &self,
        _req: &Request,
        ino: fuser::INodeNo,
        _fh: FileHandle,
        offset: u64,
        size: u32,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyData,
    ) {
        let ino_val: u64 = ino.into();
        let name = match self
            .all_files()
            .iter()
            .find(|l| file_ino(&l.name) == ino_val)
        {
            Some(l) => l.name.clone(),
            None => return reply.error(fuser::Errno::ENOENT),
        };

        match self.rt(self.store.get_binary_data(&name)) {
            Ok(data) => {
                let start = offset as usize;
                if start >= data.len() {
                    return reply.data(&[]);
                }
                let end = (start + size as usize).min(data.len());
                reply.data(&data[start..end]);
            }
            Err(_) => reply.error(fuser::Errno::EIO),
        }
    }

    fn write(
        &self,
        _req: &Request,
        ino: fuser::INodeNo,
        _fh: FileHandle,
        offset: u64,
        data: &[u8],
        _write_flags: fuser::WriteFlags,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyWrite,
    ) {
        let ino_val: u64 = ino.into();
        let mut buf = self.write_buf.write().unwrap();

        if !buf.contains_key(&ino_val) {
            let name = match self
                .all_files()
                .iter()
                .find(|l| file_ino(&l.name) == ino_val)
            {
                Some(l) => l.name.clone(),
                None => return reply.error(fuser::Errno::ENOENT),
            };
            match self.rt(self.store.get_binary_data(&name)) {
                Ok(d) => {
                    buf.insert(ino_val, d.to_vec());
                }
                Err(_) => return reply.error(fuser::Errno::ENOENT),
            }
        }

        let entry = buf.get_mut(&ino_val).unwrap();
        let off = offset as usize;
        if off + data.len() > entry.len() {
            entry.resize(off + data.len(), 0);
        }
        entry[off..off + data.len()].copy_from_slice(data);
        reply.written(data.len() as u32);
    }

    fn flush(
        &self,
        _req: &Request,
        ino: fuser::INodeNo,
        _fh: FileHandle,
        _lock_owner: LockOwner,
        reply: ReplyEmpty,
    ) {
        let ino_val: u64 = ino.into();

        let (name, data) = {
            let mut buf = self.write_buf.write().unwrap();
            match buf.remove(&ino_val) {
                Some(d) => {
                    let name = match self
                        .all_files()
                        .iter()
                        .find(|l| file_ino(&l.name) == ino_val)
                    {
                        Some(l) => l.name.clone(),
                        None => return reply.error(fuser::Errno::ENOENT),
                    };
                    (name, d)
                }
                None => return reply.ok(),
            }
        };

        match self
            .rt(self
                .store
                .put_binary_data(&name, &Bytes::from(data), true, self.compressed))
        {
            Ok(_) => reply.ok(),
            Err(_) => reply.error(fuser::Errno::EIO),
        }
    }

    fn release(
        &self,
        _req: &Request,
        ino: fuser::INodeNo,
        _fh: FileHandle,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        flush_called: bool,
        reply: ReplyEmpty,
    ) {
        let ino_val: u64 = ino.into();

        if flush_called {
            // flush already persisted, just ensure buffer is clean
            self.write_buf.write().unwrap().remove(&ino_val);
            return reply.ok();
        }

        let (name, data) = {
            let mut buf = self.write_buf.write().unwrap();
            match buf.remove(&ino_val) {
                Some(d) => {
                    let name = match self
                        .all_files()
                        .iter()
                        .find(|l| file_ino(&l.name) == ino_val)
                    {
                        Some(l) => l.name.clone(),
                        None => return reply.error(fuser::Errno::ENOENT),
                    };
                    (name, d)
                }
                None => return reply.ok(),
            }
        };

        match self
            .rt(self
                .store
                .put_binary_data(&name, &Bytes::from(data), true, self.compressed))
        {
            Ok(_) => reply.ok(),
            Err(_) => reply.error(fuser::Errno::EIO),
        }
    }

    fn fsync(
        &self,
        _req: &Request,
        ino: fuser::INodeNo,
        _fh: FileHandle,
        _datasync: bool,
        reply: ReplyEmpty,
    ) {
        let ino_val: u64 = ino.into();

        let (name, data) = {
            let mut buf = self.write_buf.write().unwrap();
            match buf.remove(&ino_val) {
                Some(d) => {
                    let name = match self
                        .all_files()
                        .iter()
                        .find(|l| file_ino(&l.name) == ino_val)
                    {
                        Some(l) => l.name.clone(),
                        None => return reply.error(fuser::Errno::ENOENT),
                    };
                    (name, d)
                }
                None => return reply.ok(),
            }
        };

        match self
            .rt(self
                .store
                .put_binary_data(&name, &Bytes::from(data), true, self.compressed))
        {
            Ok(_) => reply.ok(),
            Err(_) => reply.error(fuser::Errno::EIO),
        }
    }

    fn create(
        &self,
        _req: &Request,
        parent: fuser::INodeNo,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        _flags: i32,
        reply: ReplyCreate,
    ) {
        let parent_val: u64 = parent.into();
        let mode = S_IFREG | (_mode & 0o777);

        let parent_path = if parent_val == ROOT_INO {
            String::new()
        } else {
            match self.resolve_dir_path(parent_val) {
                Some(p) => p,
                None => return reply.error(fuser::Errno::ENOENT),
            }
        };

        let name_str = match name.to_str() {
            Some(n) => n,
            None => return reply.error(fuser::Errno::EINVAL),
        };

        let full_path = if parent_path.is_empty() {
            name_str.to_string()
        } else {
            format!("{}/{}", parent_path, name_str)
        };

        match self
            .rt(self
                .store
                .put_binary_data(&full_path, &Bytes::new(), false, self.compressed))
        {
            Ok(_) => {
                let _ = self.rt(self.store.set_file_mode(&full_path, mode));
                let ino = file_ino(&full_path);
                let attr = make_attr(ino, 0, kind_from_mode(mode), perm_from_mode(mode));
                reply.created(
                    &TTL,
                    &attr,
                    fuser::Generation(0),
                    FileHandle(0),
                    fuser::FopenFlags::empty(),
                );
            }
            Err(_) => reply.error(fuser::Errno::EIO),
        }
    }

    fn symlink(
        &self,
        _req: &Request,
        parent: fuser::INodeNo,
        name: &OsStr,
        link: &Path,
        reply: ReplyEntry,
    ) {
        let parent_val: u64 = parent.into();

        let parent_path = if parent_val == ROOT_INO {
            String::new()
        } else {
            match self.resolve_dir_path(parent_val) {
                Some(p) => p,
                None => return reply.error(fuser::Errno::ENOENT),
            }
        };

        let name_str = match name.to_str() {
            Some(n) => n,
            None => return reply.error(fuser::Errno::EINVAL),
        };

        let full_path = if parent_path.is_empty() {
            name_str.to_string()
        } else {
            format!("{}/{}", parent_path, name_str)
        };

        let target = match link.to_str() {
            Some(t) => t.as_bytes(),
            None => return reply.error(fuser::Errno::EINVAL),
        };

        match self
            .rt(self
                .store
                .put_binary_data(&full_path, &Bytes::from(target), false, self.compressed))
        {
            Ok(_) => {
                let _ = self.rt(self.store.set_file_mode(&full_path, MODE_SYMLINK));
                let ino = file_ino(&full_path);
                let attr = make_attr(ino, target.len() as u64, FileType::Symlink, 0o777);
                reply.entry(&TTL, &attr, fuser::Generation(0));
            }
            Err(_) => reply.error(fuser::Errno::EIO),
        }
    }

    fn readlink(
        &self,
        _req: &Request,
        ino: fuser::INodeNo,
        reply: ReplyData,
    ) {
        let ino_val: u64 = ino.into();
        let name = match self
            .all_files()
            .iter()
            .find(|l| file_ino(&l.name) == ino_val)
        {
            Some(l) => l.name.clone(),
            None => return reply.error(fuser::Errno::ENOENT),
        };

        match self.rt(self.store.get_binary_data(&name)) {
            Ok(data) => reply.data(&data),
            Err(_) => reply.error(fuser::Errno::EIO),
        }
    }

    fn mkdir(
        &self,
        _req: &Request,
        parent: fuser::INodeNo,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        let parent_val: u64 = parent.into();
        let mode = S_IFDIR | (_mode & 0o777);

        let parent_path = if parent_val == ROOT_INO {
            String::new()
        } else {
            match self.resolve_dir_path(parent_val) {
                Some(p) => p,
                None => return reply.error(fuser::Errno::ENOENT),
            }
        };

        let name_str = match name.to_str() {
            Some(n) => n,
            None => return reply.error(fuser::Errno::EINVAL),
        };

        let dir_path = if parent_path.is_empty() {
            name_str.to_string()
        } else {
            format!("{}/{}", parent_path, name_str)
        };

        match self.rt(self.store.mkdir(&dir_path, &parent_path)) {
            Ok(_) => {
                let _ = self.rt(self.store.set_dir_mode(&dir_path, mode));
                let ino = dir_ino(&dir_path);
                let attr = make_attr(ino, 0, FileType::Directory, perm_from_mode(mode));
                reply.entry(&TTL, &attr, fuser::Generation(0));
            }
            Err(_) => reply.error(fuser::Errno::EIO),
        }
    }

    fn rmdir(
        &self,
        _req: &Request,
        parent: fuser::INodeNo,
        name: &OsStr,
        reply: ReplyEmpty,
    ) {
        let parent_val: u64 = parent.into();

        let parent_path = if parent_val == ROOT_INO {
            String::new()
        } else {
            match self.resolve_dir_path(parent_val) {
                Some(p) => p,
                None => return reply.error(fuser::Errno::ENOENT),
            }
        };

        let name_str = match name.to_str() {
            Some(n) => n,
            None => return reply.error(fuser::Errno::EINVAL),
        };

        let dir_path = if parent_path.is_empty() {
            name_str.to_string()
        } else {
            format!("{}/{}", parent_path, name_str)
        };

        let child_dirs = self.rt(self.store.list_child_dirs(&dir_path)).unwrap_or_default();
        if !child_dirs.is_empty() {
            return reply.error(fuser::Errno::ENOTEMPTY);
        }

        let prefix = &format!("{}/", dir_path);
        let files = self.all_files();
        let has_files = files.iter().any(|l| l.name.starts_with(prefix));
        if has_files {
            return reply.error(fuser::Errno::ENOTEMPTY);
        }

        match self.rt(self.store.rmdir(&dir_path)) {
            Ok(_) => reply.ok(),
            Err(_) => reply.error(fuser::Errno::ENOENT),
        }
    }

    fn unlink(
        &self,
        _req: &Request,
        parent: fuser::INodeNo,
        name: &OsStr,
        reply: ReplyEmpty,
    ) {
        let parent_val: u64 = parent.into();

        let parent_path = if parent_val == ROOT_INO {
            String::new()
        } else {
            match self.resolve_dir_path(parent_val) {
                Some(p) => p,
                None => return reply.error(fuser::Errno::ENOENT),
            }
        };

        let name_str = match name.to_str() {
            Some(n) => n,
            None => return reply.error(fuser::Errno::EINVAL),
        };

        let full_path = if parent_path.is_empty() {
            name_str.to_string()
        } else {
            format!("{}/{}", parent_path, name_str)
        };

        match self.rt(self.store.delete(&full_path, false)) {
            Ok(_) => reply.ok(),
            Err(_) => reply.error(fuser::Errno::ENOENT),
        }
    }

    fn rename(
        &self,
        _req: &Request,
        parent: fuser::INodeNo,
        name: &OsStr,
        newparent: fuser::INodeNo,
        newname: &OsStr,
        _flags: fuser::RenameFlags,
        reply: ReplyEmpty,
    ) {
        let (parent_val, newparent_val): (u64, u64) = (parent.into(), newparent.into());

        let parent_path = if parent_val == ROOT_INO {
            String::new()
        } else {
            match self.resolve_dir_path(parent_val) {
                Some(p) => p,
                None => return reply.error(fuser::Errno::ENOENT),
            }
        };

        let new_parent_path = if newparent_val == ROOT_INO {
            String::new()
        } else {
            match self.resolve_dir_path(newparent_val) {
                Some(p) => p,
                None => return reply.error(fuser::Errno::ENOENT),
            }
        };

        let (old_name, new_name) = match (name.to_str(), newname.to_str()) {
            (Some(o), Some(n)) => (o, n),
            _ => return reply.error(fuser::Errno::EINVAL),
        };

        let old_path = if parent_path.is_empty() {
            old_name.to_string()
        } else {
            format!("{}/{}", parent_path, old_name)
        };

        let new_path = if new_parent_path.is_empty() {
            new_name.to_string()
        } else {
            format!("{}/{}", new_parent_path, new_name)
        };

        let data = match self.rt(self.store.get_binary_data(&old_path)) {
            Ok(d) => d,
            Err(_) => return reply.error(fuser::Errno::ENOENT),
        };

        if self
            .rt(self
                .store
                .put_binary_data(&new_path, &data, true, self.compressed))
            .is_err()
        {
            return reply.error(fuser::Errno::EIO);
        }
        let _ = self.rt(self.store.delete(&old_path, false));
        reply.ok();
    }

    fn fallocate(
        &self,
        _req: &Request,
        _ino: fuser::INodeNo,
        _fh: FileHandle,
        _offset: u64,
        _length: u64,
        _mode: i32,
        reply: ReplyEmpty,
    ) {
        reply.ok();
    }

    fn lseek(
        &self,
        _req: &Request,
        ino: fuser::INodeNo,
        _fh: FileHandle,
        offset: i64,
        whence: i32,
        reply: ReplyLseek,
    ) {
        let ino_val: u64 = ino.into();

        let file_size = match self.all_files().iter().find(|l| file_ino(&l.name) == ino_val) {
            Some(l) => {
                let buf = self.write_buf.read().unwrap();
                buf.get(&ino_val)
                    .map(|d| d.len())
                    .unwrap_or_else(|| {
                        self.rt(self.store.get_binary_data(&l.name))
                            .map(|d| d.len())
                            .unwrap_or(0)
                    })
            }
            None => 0,
        } as i64;

        // SEEK_END
        if whence == 2 {
            reply.offset(file_size.wrapping_add(offset).max(0) as u64);
            return;
        }
        // SEEK_DATA: no holes, data starts at this offset if within file
        if whence == 3 {
            let o = offset.max(0) as i64;
            if o > file_size {
                reply.offset(o as u64);  // ENXIO handled by caller
            } else {
                reply.offset(o as u64);
            }
            return;
        }
        // SEEK_HOLE: no holes, hole at EOF
        if whence == 4 {
            reply.offset(file_size as u64);
            return;
        }
        reply.offset(0);
    }

    fn statfs(
        &self,
        _req: &Request,
        _ino: fuser::INodeNo,
        reply: ReplyStatfs,
    ) {
        let files = self.all_files().len() as u64;
        reply.statfs(0, 0, 0, files, 0, 4096, 255, 4096);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_ino_deterministic() {
        let a = file_ino("hello.txt");
        let b = file_ino("hello.txt");
        assert_eq!(a, b);
    }

    #[test]
    fn test_file_ino_different() {
        let a = file_ino("a.txt");
        let b = file_ino("b.txt");
        assert_ne!(a, b);
    }

    #[test]
    fn test_file_ino_not_reserved() {
        let ino = file_ino("anything");
        assert_ne!(ino, 0);
        assert_ne!(ino, ROOT_INO);
    }

    #[test]
    fn test_dir_ino_not_reserved() {
        let ino = dir_ino("somedir");
        assert_ne!(ino, 0);
        assert_ne!(ino, ROOT_INO);
    }

    #[test]
    fn test_file_dir_ino_distinct() {
        let f = file_ino("docs");
        let d = dir_ino("docs");
        assert_ne!(f, d, "file and dir with same name should have different inodes");
    }

    #[test]
    fn test_dir_ino_deterministic() {
        let a = dir_ino("a/b/c");
        let b = dir_ino("a/b/c");
        assert_eq!(a, b);
    }

    #[test]
    fn test_make_attr_regular_file() {
        let attr = make_attr(42, 1024, FileType::RegularFile, 0o644);
        assert_eq!(u64::from(attr.ino), 42);
        assert_eq!(attr.size, 1024);
        assert_eq!(attr.kind, FileType::RegularFile);
        assert_eq!(attr.perm, 0o644);
        assert_eq!(attr.nlink, 1);
    }

    #[test]
    fn test_make_attr_directory() {
        let attr = make_attr(ROOT_INO, 0, FileType::Directory, 0o755);
        assert_eq!(u64::from(attr.ino), ROOT_INO);
        assert_eq!(attr.kind, FileType::Directory);
        assert_eq!(attr.perm, 0o755);
        assert_eq!(attr.nlink, 2);
    }

    #[test]
    fn test_direct_child_name() {
        assert_eq!(direct_child_name("file.txt", ""), Some("file.txt".into()));
        assert_eq!(direct_child_name("docs/readme.md", "docs"), Some("readme.md".into()));
        assert_eq!(direct_child_name("docs/api/v1.md", "docs"), None);
        assert_eq!(direct_child_name("docs/api/v1.md", "docs/api"), Some("v1.md".into()));
        assert_eq!(direct_child_name("other/file.txt", "docs"), None);
    }

    #[test]
    fn test_file_ino_consistency_across_names() {
        let names: Vec<String> = (0..100).map(|i| format!("file_{}.txt", i)).collect();
        let mut inos: Vec<u64> = names.iter().map(|n| file_ino(n)).collect();
        inos.sort();
        inos.dedup();
        assert_eq!(inos.len(), names.len());
    }
}
