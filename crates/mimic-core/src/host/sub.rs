//! A [`FileHost`] showing one subfolder of a shared folder as its root:
//! a device that owns a part of the machine's folder (the SD2IEC's card is
//! `/SD2IEC`) sees only that part. The subfolder is created when it is
//! first listed or written to.

use super::{
    check_path, lock, DirEntry, FileHandle, FileHost, FileStat, FsError, OpenMode, SharedFiles,
};

pub struct SubFileHost {
    inner: SharedFiles,
    /// Canonical path of the subfolder in `inner`.
    prefix: String,
}

impl SubFileHost {
    /// The subfolder at canonical `prefix` of `inner`.
    pub fn new(inner: SharedFiles, prefix: &str) -> Result<Self, FsError> {
        check_path(prefix)?;
        Ok(Self {
            inner,
            prefix: prefix.trim_end_matches('/').to_string(),
        })
    }

    fn map(&self, path: &str) -> Result<String, FsError> {
        check_path(path)?;
        Ok(if path == "/" {
            if self.prefix.is_empty() {
                "/".into()
            } else {
                self.prefix.clone()
            }
        } else {
            format!("{}{path}", self.prefix)
        })
    }

    /// Create the subfolder (and its parents) if it is missing.
    fn ensure_root(&self, fs: &mut dyn FileHost) {
        let mut at = String::new();
        for part in self.prefix.split('/').filter(|p| !p.is_empty()) {
            at.push('/');
            at.push_str(part);
            if fs.stat(&at).is_err() {
                let _ = fs.mkdir(&at);
            }
        }
    }
}

impl FileHost for SubFileHost {
    fn open(&mut self, path: &str, mode: OpenMode) -> Result<FileHandle, FsError> {
        let p = self.map(path)?;
        let mut fs = lock(&self.inner);
        if mode.write || mode.create {
            self.ensure_root(&mut *fs);
        }
        fs.open(&p, mode)
    }
    fn read(&mut self, h: FileHandle, buf: &mut [u8]) -> Result<usize, FsError> {
        lock(&self.inner).read(h, buf)
    }
    fn write(&mut self, h: FileHandle, data: &[u8]) -> Result<usize, FsError> {
        lock(&self.inner).write(h, data)
    }
    fn seek(&mut self, h: FileHandle, pos: u64) -> Result<(), FsError> {
        lock(&self.inner).seek(h, pos)
    }
    fn fstat(&mut self, h: FileHandle) -> Result<FileStat, FsError> {
        lock(&self.inner).fstat(h)
    }
    fn close(&mut self, h: FileHandle) {
        lock(&self.inner).close(h);
    }
    fn stat(&mut self, path: &str) -> Result<FileStat, FsError> {
        let p = self.map(path)?;
        lock(&self.inner).stat(&p)
    }
    fn list_dir(&mut self, path: &str) -> Result<Vec<DirEntry>, FsError> {
        let p = self.map(path)?;
        let mut fs = lock(&self.inner);
        if path == "/" {
            self.ensure_root(&mut *fs);
        }
        fs.list_dir(&p)
    }
    fn mkdir(&mut self, path: &str) -> Result<(), FsError> {
        let p = self.map(path)?;
        let mut fs = lock(&self.inner);
        self.ensure_root(&mut *fs);
        fs.mkdir(&p)
    }
    fn delete(&mut self, path: &str) -> Result<(), FsError> {
        if path == "/" {
            return Err(FsError::Denied);
        }
        let p = self.map(path)?;
        lock(&self.inner).delete(&p)
    }
    fn rename(&mut self, from: &str, to: &str) -> Result<(), FsError> {
        if from == "/" || to == "/" {
            return Err(FsError::Denied);
        }
        let (f, t) = (self.map(from)?, self.map(to)?);
        lock(&self.inner).rename(&f, &t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::{HostServices, MemFileHost};

    #[test]
    fn sees_only_its_subfolder_and_creates_it() {
        let mut mem = MemFileHost::new();
        mem.insert("/other.txt", b"x".to_vec()).unwrap();
        let shared = HostServices::new(None, Some(Box::new(mem))).files.unwrap();
        let mut sub = SubFileHost::new(shared.clone(), "/SD2IEC").unwrap();
        assert!(sub.list_dir("/").unwrap().is_empty());
        assert!(lock(&shared).stat("/SD2IEC").unwrap().is_dir);
        let h = sub
            .open(
                "/a.prg",
                OpenMode {
                    write: true,
                    create: true,
                    ..OpenMode::default()
                },
            )
            .unwrap();
        sub.write(h, b"hi").unwrap();
        sub.close(h);
        assert_eq!(lock(&shared).stat("/SD2IEC/a.prg").unwrap().size, 2);
        assert_eq!(sub.list_dir("/").unwrap()[0].name, "a.prg");
        assert!(sub.stat("/other.txt").is_err());
        assert_eq!(sub.delete("/"), Err(FsError::Denied));
    }
}
