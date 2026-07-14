use std::{
    ffi::OsStr,
    hash::{Hash, Hasher},
    path::Path,
    sync::Arc,
    time::{Duration, UNIX_EPOCH},
};

use fuser::{FileAttr, FileType, Filesystem, ReplyAttr, ReplyData, ReplyDirectory, ReplyEmpty, ReplyEntry, Request};
use linabase::service::StoreManager;

fn ino_from_name(name: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut h);
    h.finish()
}

const ROOT_INO: u64 = 1;
const TTL: Duration = Duration::from_secs(1);

fn make_attr(ino: u64, size: u64, kind: FileType, perm: u16) -> FileAttr {
    FileAttr {
        ino,
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

pub struct LinaFs {
    store: Arc<StoreManager>,
}

impl LinaFs {
    async fn new(root: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let store = StoreManager::new(root).await?;
        Ok(Self { store: Arc::new(store) })
    }

    fn rt<F, T>(&self, f: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        tokio::runtime::Handle::current().block_on(f)
    }

    fn all_files(&self) -> Vec<linabase::dao::Link> {
        self.rt(self.store.list("*", 0, false, true)).unwrap_or_default()
    }
}

impl Filesystem for LinaFs {
    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        if parent != ROOT_INO {
            return reply.error(libc::ENOENT);
        }
        let name = match name.to_str() {
            Some(n) => n,
            None => return reply.error(libc::EINVAL),
        };

        match self.rt(self.store.get_binary_data(name)) {
            Ok(data) => {
                let ino = ino_from_name(name);
                let attr = make_attr(ino, data.len() as u64, FileType::RegularFile, 0o444);
                reply.entry(&TTL, &attr, 0);
            }
            Err(_) => reply.error(libc::ENOENT),
        }
    }

    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
        if ino == ROOT_INO {
            let attr = make_attr(ROOT_INO, 0, FileType::Directory, 0o755);
            return reply.attr(&TTL, &attr);
        }

        for link in self.all_files() {
            if ino_from_name(&link.name) == ino {
                match self.rt(self.store.get_binary_data(&link.name)) {
                    Ok(data) => {
                        let attr = make_attr(ino, data.len() as u64, FileType::RegularFile, 0o444);
                        return reply.attr(&TTL, &attr);
                    }
                    Err(_) => return reply.error(libc::EIO),
                }
            }
        }
        reply.error(libc::ENOENT);
    }

    fn readdir(&mut self, _req: &Request, ino: u64, _fh: u64, offset: i64, mut reply: ReplyDirectory) {
        if ino != ROOT_INO {
            return reply.error(libc::ENOENT);
        }

        let entries: Vec<(u64, FileType, String)> = std::iter::once((ROOT_INO, FileType::Directory, ".".into()))
            .chain(std::iter::once((ROOT_INO, FileType::Directory, "..".into())))
            .chain(self.all_files().into_iter().map(|link| {
                (ino_from_name(&link.name), FileType::RegularFile, link.name)
            }))
            .collect();

        for (i, (ino, kind, name)) in entries.iter().enumerate().skip(offset as usize) {
            if reply.add(*ino, (i + 1) as i64, *kind, name) {
                break;
            }
        }
        reply.ok();
    }

    fn read(&mut self, _req: &Request, ino: u64, _fh: u64, offset: i64, size: u32, _flags: i32, _lock_owner: Option<u64>, reply: ReplyData) {
        let name = match self.all_files().iter().find(|l| ino_from_name(&l.name) == ino) {
            Some(l) => l.name.clone(),
            None => return reply.error(libc::ENOENT),
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
            Err(_) => reply.error(libc::EIO),
        }
    }

    fn write(&mut self, _req: &Request, ino: u64, _fh: u64, offset: i64, data: &[u8], _write_flags: u32, _flags: i32, _lock_owner: Option<u64>, reply: ReplyWrite) {
        let name = match self.all_files().iter().find(|l| ino_from_name(&l.name) == ino) {
            Some(l) => l.name.clone(),
            None => return reply.error(libc::ENOENT),
        };

        match self.rt(self.store.get_binary_data(&name)) {
            Ok(existing) => {
                let off = offset as usize;
                let mut new = existing.to_vec();
                if off + data.len() > new.len() {
                    new.resize(off + data.len(), 0);
                }
                new[off..off + data.len()].copy_from_slice(data);
                match self.rt(self.store.put_binary_data(&name, &new.into(), true, false)) {
                    Ok(_) => reply.written(data.len() as u32),
                    Err(_) => reply.error(libc::EIO),
                }
            }
            Err(_) => reply.error(libc::ENOENT),
        }
    }

    fn create(&mut self, _req: &Request, parent: u64, name: &OsStr, _mode: u32, _flags: i32, reply: ReplyEntry) {
        if parent != ROOT_INO {
            return reply.error(libc::ENOENT);
        }
        let name = match name.to_str() {
            Some(n) => n,
            None => return reply.error(libc::EINVAL),
        };

        match self.rt(self.store.put_binary_data(name, &vec![].into(), false, false)) {
            Ok(_) => {
                let ino = ino_from_name(name);
                let attr = make_attr(ino, 0, FileType::RegularFile, 0o644);
                reply.entry(&TTL, &attr, 0);
            }
            Err(_) => reply.error(libc::EIO),
        }
    }

    fn unlink(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        if parent != ROOT_INO {
            return reply.error(libc::ENOENT);
        }
        let name = match name.to_str() {
            Some(n) => n,
            None => return reply.error(libc::EINVAL),
        };

        match self.rt(self.store.delete(name, false)) {
            Ok(_) => reply.ok(),
            Err(_) => reply.error(libc::ENOENT),
        }
    }

    fn rename(&mut self, _req: &Request, parent: u64, name: &OsStr, newparent: u64, newname: &OsStr, reply: ReplyEmpty) {
        if parent != ROOT_INO || newparent != ROOT_INO {
            return reply.error(libc::ENOENT);
        }
        let (old, new) = match (name.to_str(), newname.to_str()) {
            (Some(o), Some(n)) => (o, n),
            _ => return reply.error(libc::EINVAL),
        };

        let data = match self.rt(self.store.get_binary_data(old)) {
            Ok(d) => d,
            Err(_) => return reply.error(libc::ENOENT),
        };

        if self.rt(self.store.put_binary_data(new, &data, true, false)).is_err() {
            return reply.error(libc::EIO);
        }
        let _ = self.rt(self.store.delete(old, false));
        reply.ok();
    }
}

pub fn mount_inner(root: &str, mount_point: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let rt = tokio::runtime::Runtime::new()?;
    let fs = rt.block_on(LinaFs::new(root))?;
    fuser::mount2(fs, Path::new(mount_point), &[])?;
    Ok(())
}
