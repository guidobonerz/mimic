//! A [`FileHost`] that hangs further folders into a root folder, the way
//! the Ultimate's file system shows its drives (`/SD`, `/Usb0`, `/Usb1`,
//! `/Flash`, `/Temp`) side by side in the root.
//!
//! Each mount point is a directory of the root named after the mount; a
//! path below it goes to the mounted folder with the mount's name cut off
//! (`/Usb0/games/a.prg` → `/games/a.prg` in the folder mounted as `Usb0`).
//! Names match case-insensitively (the Ultimate's FAT media do), listings
//! show them as given. A mount hides an entry of the same name in the
//! root folder. Mount points themselves cannot be deleted, renamed or
//! created, and a rename cannot move between folders.

use std::collections::BTreeMap;

use super::{check_path, DirEntry, FileHandle, FileHost, FileStat, FsError, OpenMode};

/// Root folder plus mounted folders (see the module documentation).
pub struct MountFileHost {
    /// The root folder; without one the root holds only the mounts.
    root: Option<Box<dyn FileHost>>,
    mounts: Vec<(String, Box<dyn FileHost>)>,
    /// Outer handle → (folder: 0 = root, n = mount n − 1; inner handle).
    open: BTreeMap<FileHandle, (usize, FileHandle)>,
    next: FileHandle,
}

impl MountFileHost {
    pub fn new(root: Option<Box<dyn FileHost>>) -> Self {
        Self {
            root,
            mounts: Vec::new(),
            open: BTreeMap::new(),
            next: 1,
        }
    }

    /// Hang `folder` into the root as directory `name`. The name is one
    /// path component (no `/`, `\`, `:`, not `.`/`..`) and not mounted yet.
    pub fn mount(&mut self, name: &str, folder: Box<dyn FileHost>) -> Result<(), FsError> {
        if check_path(&format!("/{name}"))?.len() != 1 {
            return Err(FsError::InvalidPath);
        }
        if self.find(name).is_some() {
            return Err(FsError::Exists);
        }
        self.mounts.push((name.to_string(), folder));
        Ok(())
    }

    /// The mount names in the order mounted.
    pub fn mount_names(&self) -> impl Iterator<Item = &str> {
        self.mounts.iter().map(|(n, _)| n.as_str())
    }

    fn find(&self, name: &str) -> Option<usize> {
        self.mounts
            .iter()
            .position(|(n, _)| n.eq_ignore_ascii_case(name))
    }

    /// The folder and its path for canonical `path`; `None` as the path
    /// for the root itself.
    fn resolve(&self, path: &str) -> Result<(usize, Option<String>), FsError> {
        let parts = check_path(path)?;
        let Some(first) = parts.first() else {
            return Ok((0, None));
        };
        Ok(match self.find(first) {
            Some(m) => (m + 1, Some(format!("/{}", parts[1..].join("/")))),
            None => (0, Some(path.to_string())),
        })
    }

    fn folder(&mut self, i: usize) -> Result<&mut (dyn FileHost + 'static), FsError> {
        match i {
            0 => self.root.as_deref_mut().ok_or(FsError::Unavailable),
            m => Ok(&mut *self.mounts[m - 1].1),
        }
    }

    /// The folder and path for an operation that changes `path`: the
    /// root and the mount points themselves are refused.
    fn resolve_inner(&self, path: &str) -> Result<(usize, String), FsError> {
        match self.resolve(path)? {
            (_, None) => Err(FsError::Denied),
            (m, Some(p)) if m > 0 && p == "/" => Err(FsError::Denied),
            (m, Some(p)) => Ok((m, p)),
        }
    }

    fn handle(
        &mut self,
        h: FileHandle,
    ) -> Result<(&mut (dyn FileHost + 'static), FileHandle), FsError> {
        let &(i, inner) = self.open.get(&h).ok_or(FsError::BadHandle)?;
        Ok((self.folder(i)?, inner))
    }
}

/// How a mount point looks in the root: a directory, whatever its
/// folder reports (it may not exist yet).
fn mount_stat() -> FileStat {
    FileStat {
        is_dir: true,
        ..FileStat::default()
    }
}

impl FileHost for MountFileHost {
    fn open(&mut self, path: &str, mode: OpenMode) -> Result<FileHandle, FsError> {
        let (i, p) = match self.resolve(path)? {
            (_, None) => return Err(FsError::IsADirectory),
            (i, Some(p)) => (i, p),
        };
        let inner = self.folder(i)?.open(&p, mode)?;
        let h = self.next;
        self.next = self.next.wrapping_add(1).max(1);
        self.open.insert(h, (i, inner));
        Ok(h)
    }

    fn read(&mut self, h: FileHandle, buf: &mut [u8]) -> Result<usize, FsError> {
        let (f, inner) = self.handle(h)?;
        f.read(inner, buf)
    }

    fn write(&mut self, h: FileHandle, data: &[u8]) -> Result<usize, FsError> {
        let (f, inner) = self.handle(h)?;
        f.write(inner, data)
    }

    fn seek(&mut self, h: FileHandle, pos: u64) -> Result<(), FsError> {
        let (f, inner) = self.handle(h)?;
        f.seek(inner, pos)
    }

    fn fstat(&mut self, h: FileHandle) -> Result<FileStat, FsError> {
        let (f, inner) = self.handle(h)?;
        f.fstat(inner)
    }

    fn close(&mut self, h: FileHandle) {
        if let Ok((f, inner)) = self.handle(h) {
            f.close(inner);
        }
        self.open.remove(&h);
    }

    fn stat(&mut self, path: &str) -> Result<FileStat, FsError> {
        match self.resolve(path)? {
            (_, None) => Ok(mount_stat()),
            (m, Some(p)) if m > 0 && p == "/" => Ok(self
                .folder(m)?
                .stat("/")
                .map(|s| FileStat { is_dir: true, ..s })
                .unwrap_or_else(|_| mount_stat())),
            (i, Some(p)) => self.folder(i)?.stat(&p),
        }
    }

    fn list_dir(&mut self, path: &str) -> Result<Vec<DirEntry>, FsError> {
        match self.resolve(path)? {
            (_, None) => {
                // The root folder's entries (none if it is missing or not
                // there yet), the mounts in place of same-named ones.
                let mut list: Vec<DirEntry> = match self.root.as_deref_mut() {
                    Some(r) => r.list_dir("/").unwrap_or_default(),
                    None => Vec::new(),
                };
                list.retain(|e| self.find(&e.name).is_none());
                for m in 1..=self.mounts.len() {
                    let stat = self.stat(&format!("/{}", self.mounts[m - 1].0))?;
                    list.push(DirEntry {
                        name: self.mounts[m - 1].0.clone(),
                        stat,
                    });
                }
                list.sort_by(|a, b| a.name.cmp(&b.name));
                Ok(list)
            }
            (m, Some(p)) if m > 0 && p == "/" => match self.folder(m)?.list_dir("/") {
                // A mounted folder that does not exist yet is empty.
                Err(FsError::NotFound) => Ok(Vec::new()),
                r => r,
            },
            (i, Some(p)) => self.folder(i)?.list_dir(&p),
        }
    }

    fn mkdir(&mut self, path: &str) -> Result<(), FsError> {
        let (i, p) = self.resolve_inner(path)?;
        self.folder(i)?.mkdir(&p)
    }

    fn delete(&mut self, path: &str) -> Result<(), FsError> {
        let (i, p) = self.resolve_inner(path)?;
        self.folder(i)?.delete(&p)
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<(), FsError> {
        let (i, a) = self.resolve_inner(from)?;
        let (j, b) = self.resolve_inner(to)?;
        if i != j {
            return Err(FsError::Denied);
        }
        self.folder(i)?.rename(&a, &b)
    }
}

#[cfg(test)]
mod tests {
    use super::super::MemFileHost;
    use super::*;

    fn host() -> MountFileHost {
        let mut root = MemFileHost::new();
        root.insert("/readme.txt", b"root".to_vec()).unwrap();
        root.insert("/usb0/hidden.prg", b"x".to_vec()).unwrap();
        let mut usb = MemFileHost::new();
        usb.insert("/games/a.prg", b"usb".to_vec()).unwrap();
        let mut m = MountFileHost::new(Some(Box::new(root)));
        m.mount("Usb0", Box::new(usb)).unwrap();
        m.mount("SD", Box::new(MemFileHost::new())).unwrap();
        m
    }

    fn names(list: Vec<DirEntry>) -> Vec<String> {
        list.into_iter().map(|e| e.name).collect()
    }

    #[test]
    fn mounts_appear_in_the_root_and_hide_same_named_entries() {
        let mut m = host();
        assert_eq!(
            names(m.list_dir("/").unwrap()),
            ["SD", "Usb0", "readme.txt"]
        );
        assert!(m.stat("/usb0").unwrap().is_dir);
        assert_eq!(names(m.list_dir("/USB0").unwrap()), ["games"]);
        assert_eq!(names(m.list_dir("/Usb0/games").unwrap()), ["a.prg"]);
        assert!(m.list_dir("/SD").unwrap().is_empty());
        assert_eq!(m.mount_names().collect::<Vec<_>>(), ["Usb0", "SD"]);
    }

    #[test]
    fn files_go_to_their_folder() {
        let mut m = host();
        let read = OpenMode {
            read: true,
            ..OpenMode::default()
        };
        let h = m.open("/Usb0/games/a.prg", read).unwrap();
        let r = m.open("/readme.txt", read).unwrap();
        let mut buf = [0; 8];
        assert_eq!(m.read(h, &mut buf).unwrap(), 3);
        assert_eq!(&buf[..3], b"usb");
        assert_eq!(m.read(r, &mut buf).unwrap(), 4);
        m.close(h);
        assert_eq!(m.read(h, &mut buf), Err(FsError::BadHandle));
        let w = OpenMode {
            write: true,
            create: true,
            ..OpenMode::default()
        };
        let h = m.open("/SD/new.seq", w).unwrap();
        m.write(h, b"hi").unwrap();
        m.close(h);
        assert_eq!(m.stat("/sd/new.seq").unwrap().size, 2);
        m.mkdir("/SD/dir").unwrap();
        m.rename("/SD/new.seq", "/SD/dir/new.seq").unwrap();
        assert_eq!(names(m.list_dir("/SD/dir").unwrap()), ["new.seq"]);
    }

    #[test]
    fn mount_points_are_fixed() {
        let mut m = host();
        assert_eq!(m.delete("/Usb0"), Err(FsError::Denied));
        assert_eq!(m.mkdir("/SD"), Err(FsError::Denied));
        assert_eq!(m.rename("/Usb0", "/x"), Err(FsError::Denied));
        assert_eq!(
            m.rename("/Usb0/games/a.prg", "/SD/a.prg"),
            Err(FsError::Denied)
        );
        assert_eq!(
            m.mount("sd", Box::new(MemFileHost::new())),
            Err(FsError::Exists)
        );
        assert_eq!(
            m.mount("a/b", Box::new(MemFileHost::new())),
            Err(FsError::InvalidPath)
        );
        assert_eq!(
            m.mount("..", Box::new(MemFileHost::new())),
            Err(FsError::InvalidPath)
        );
    }

    #[test]
    fn without_a_root_folder_only_the_mounts_are_there() {
        let mut m = MountFileHost::new(None);
        m.mount("Usb1", Box::new(MemFileHost::new())).unwrap();
        assert_eq!(names(m.list_dir("/").unwrap()), ["Usb1"]);
        assert_eq!(m.stat("/x"), Err(FsError::Unavailable));
    }
}
