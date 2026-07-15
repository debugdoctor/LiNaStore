use std::{
    ffi::OsStr,
    hash::{Hash, Hasher},
    sync::Arc,
    time::{Duration, UNIX_EPOCH},
};

use fuser::{
    FileAttr, FileHandle, FileType, Filesystem, ReplyAttr, ReplyCreate, ReplyData,
    ReplyDirectory, ReplyEmpty, ReplyEntry, ReplyWrite, Request,
};
use linabase::{dao::Link, service::StoreManager};

fn ino_from_name(name: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut h);
    h.finish()
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

pub struct LinaFs {
    store: Arc<StoreManager>,
    rt: tokio::runtime::Handle,
}

impl LinaFs {
    pub async fn new(root: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let store = StoreManager::new(root).await?;
        Ok(Self {
            store: Arc::new(store),
            rt: tokio::runtime::Handle::current(),
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
}

impl Filesystem for LinaFs {
    fn lookup(&self, _req: &Request, parent: fuser::INodeNo, name: &OsStr, reply: ReplyEntry) {
        let parent: u64 = parent.into();
        if parent != ROOT_INO {
            return reply.error(fuser::Errno::ENOENT);
        }
        let name = match name.to_str() {
            Some(n) => n,
            None => return reply.error(fuser::Errno::EINVAL),
        };

        match self.rt(self.store.get_binary_data(name)) {
            Ok(data) => {
                let ino = ino_from_name(name);
                let attr = make_attr(ino, data.len() as u64, FileType::RegularFile, 0o444);
                reply.entry(&TTL, &attr, fuser::Generation(0));
            }
            Err(_) => reply.error(fuser::Errno::ENOENT),
        }
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

        for link in self.all_files() {
            if ino_from_name(&link.name) == ino_val {
                match self.rt(self.store.get_binary_data(&link.name)) {
                    Ok(data) => {
                        let attr =
                            make_attr(ino_val, data.len() as u64, FileType::RegularFile, 0o444);
                        return reply.attr(&TTL, &attr);
                    }
                    Err(_) => return reply.error(fuser::Errno::EIO),
                }
            }
        }
        reply.error(fuser::Errno::ENOENT);
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
        if ino_val != ROOT_INO {
            return reply.error(fuser::Errno::ENOENT);
        }

        let entries: Vec<(u64, FileType, String)> =
            std::iter::once((ROOT_INO, FileType::Directory, ".".into()))
                .chain(std::iter::once((
                    ROOT_INO,
                    FileType::Directory,
                    "..".into(),
                )))
                .chain(self.all_files().into_iter().map(|link| {
                    (
                        ino_from_name(&link.name),
                        FileType::RegularFile,
                        link.name,
                    )
                }))
                .collect();

        for (i, (ino, kind, name)) in entries.iter().enumerate().skip(offset as usize) {
            let ok = reply.add(fuser::INodeNo(*ino), (i + 1) as u64, *kind, name);
            if ok {
                break;
            }
        }
        reply.ok();
    }

    fn read(
        &self,
        _req: &Request,
        ino: fuser::INodeNo,
        _fh: FileHandle,
        offset: u64,
        size: u32,
        _flags: fuser::OpenFlags,
        _lock_owner: Option<fuser::LockOwner>,
        reply: ReplyData,
    ) {
        let ino_val: u64 = ino.into();
        let name = match self
            .all_files()
            .iter()
            .find(|l| ino_from_name(&l.name) == ino_val)
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
        _flags: fuser::OpenFlags,
        _lock_owner: Option<fuser::LockOwner>,
        reply: ReplyWrite,
    ) {
        let ino_val: u64 = ino.into();
        let name = match self
            .all_files()
            .iter()
            .find(|l| ino_from_name(&l.name) == ino_val)
        {
            Some(l) => l.name.clone(),
            None => return reply.error(fuser::Errno::ENOENT),
        };

        match self.rt(self.store.get_binary_data(&name)) {
            Ok(existing) => {
                let off = offset as usize;
                let mut new = existing.to_vec();
                if off + data.len() > new.len() {
                    new.resize(off + data.len(), 0);
                }
                new[off..off + data.len()].copy_from_slice(data);
                match self
                    .rt(self
                        .store
                        .put_binary_data(&name, &new.into(), true, false))
                {
                    Ok(_) => reply.written(data.len() as u32),
                    Err(_) => reply.error(fuser::Errno::EIO),
                }
            }
            Err(_) => reply.error(fuser::Errno::ENOENT),
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
        if parent_val != ROOT_INO {
            return reply.error(fuser::Errno::ENOENT);
        }
        let name = match name.to_str() {
            Some(n) => n,
            None => return reply.error(fuser::Errno::EINVAL),
        };

        match self
            .rt(self
                .store
                .put_binary_data(name, &vec![].into(), false, false))
        {
            Ok(_) => {
                let ino = ino_from_name(name);
                let attr = make_attr(ino, 0, FileType::RegularFile, 0o644);
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

    fn unlink(
        &self,
        _req: &Request,
        parent: fuser::INodeNo,
        name: &OsStr,
        reply: ReplyEmpty,
    ) {
        let parent_val: u64 = parent.into();
        if parent_val != ROOT_INO {
            return reply.error(fuser::Errno::ENOENT);
        }
        let name = match name.to_str() {
            Some(n) => n,
            None => return reply.error(fuser::Errno::EINVAL),
        };

        match self.rt(self.store.delete(name, false)) {
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
        let parent_val: u64 = parent.into();
        let newparent_val: u64 = newparent.into();
        if parent_val != ROOT_INO || newparent_val != ROOT_INO {
            return reply.error(fuser::Errno::ENOENT);
        }
        let (old, new) = match (name.to_str(), newname.to_str()) {
            (Some(o), Some(n)) => (o, n),
            _ => return reply.error(fuser::Errno::EINVAL),
        };

        let data = match self.rt(self.store.get_binary_data(old)) {
            Ok(d) => d,
            Err(_) => return reply.error(fuser::Errno::ENOENT),
        };

        if self
            .rt(self
                .store
                .put_binary_data(new, &data, true, false))
            .is_err()
        {
            return reply.error(fuser::Errno::EIO);
        }
        let _ = self.rt(self.store.delete(old, false));
        reply.ok();
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ino_from_name_deterministic() {
        let a = ino_from_name("hello.txt");
        let b = ino_from_name("hello.txt");
        assert_eq!(a, b);
    }

    #[test]
    fn test_ino_from_name_different() {
        let a = ino_from_name("a.txt");
        let b = ino_from_name("b.txt");
        assert_ne!(a, b);
    }

    #[test]
    fn test_ino_from_name_not_zero() {
        let ino = ino_from_name("anything");
        assert_ne!(ino, 0, "zero inode would conflict with reserved sentinels");
        assert_ne!(ino, ROOT_INO, "should not collide with root inode");
    }

    #[test]
    fn test_make_attr_regular_file() {
        let attr = make_attr(42, 1024, FileType::RegularFile, 0o644);
        assert_eq!(u64::from(attr.ino), 42);
        assert_eq!(attr.size, 1024);
        assert_eq!(attr.kind, FileType::RegularFile);
        assert_eq!(attr.perm, 0o644);
        assert_eq!(attr.nlink, 1);
        assert_eq!(attr.blksize, 4096);
        assert_eq!(attr.blocks, (1024 + 511) / 512);
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
    fn test_make_attr_zero_size() {
        let attr = make_attr(99, 0, FileType::RegularFile, 0o444);
        assert_eq!(attr.size, 0);
        assert_eq!(attr.blocks, 0);
    }

    #[test]
    fn test_ino_from_name_consistency_across_names() {
        let names: Vec<String> = (0..100).map(|i| format!("file_{}.txt", i)).collect();
        let mut inos: Vec<u64> = names.iter().map(|n| ino_from_name(n)).collect();
        inos.sort();
        inos.dedup();
        assert_eq!(
            inos.len(),
            names.len(),
            "all 100 names should produce unique inodes"
        );
    }
}
