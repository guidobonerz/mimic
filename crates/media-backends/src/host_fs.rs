//! A host folder as the sandbox of [`emu_core::host::FileHost`] (the
//! Ultimate command interface's DOS targets).
//!
//! Paths arrive canonical (`/a/b`, see [`emu_core::host::check_path`]) and
//! are checked again here: no `.`/`..`, no drive letters or backslashes,
//! no Windows device names (`CON`, `NUL`, `COM1` …) and no names ending in
//! a dot or space. Symbolic links (and Windows junctions) are never
//! followed: a path through one is refused, and listings leave them out,
//! so nothing outside the root can be reached. The root folder is created
//! on the first write.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use emu_core::host::{check_path, DirEntry, FileHandle, FileHost, FileStat, FsError, OpenMode};

struct Open {
    file: File,
    read: bool,
    write: bool,
}

pub struct FsFolder {
    root: PathBuf,
    open: BTreeMap<FileHandle, Open>,
    next: FileHandle,
}

const DEVICE_NAMES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

fn io_error(e: std::io::Error) -> FsError {
    match e.kind() {
        ErrorKind::NotFound => FsError::NotFound,
        ErrorKind::AlreadyExists => FsError::Exists,
        ErrorKind::PermissionDenied | ErrorKind::ReadOnlyFilesystem => FsError::Denied,
        ErrorKind::NotADirectory => FsError::NotADirectory,
        ErrorKind::IsADirectory => FsError::IsADirectory,
        ErrorKind::DirectoryNotEmpty => FsError::NotEmpty,
        ErrorKind::InvalidInput | ErrorKind::InvalidFilename => FsError::InvalidPath,
        _ => FsError::Io(e.to_string()),
    }
}

fn stat_of(m: &fs::Metadata) -> FileStat {
    FileStat {
        size: if m.is_dir() { 0 } else { m.len() },
        is_dir: m.is_dir(),
        read_only: m.permissions().readonly(),
        modified: m
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64),
    }
}

/// A link of any kind: symbolic link, or a Windows reparse point
/// (junction) that `is_symlink` may not report.
fn is_link(m: &fs::Metadata) -> bool {
    if m.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

impl FsFolder {
    /// The folder `root` (need not exist yet).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            open: BTreeMap::new(),
            next: 1,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The host path of canonical `path`; refuses anything that could
    /// leave the root, and links on the way.
    fn host_path(&self, path: &str) -> Result<PathBuf, FsError> {
        let parts = check_path(path)?;
        let mut p = self.root.clone();
        for part in parts {
            let upper = part.to_ascii_uppercase();
            let stem = upper.split('.').next().unwrap_or("");
            if DEVICE_NAMES.contains(&stem)
                || part.ends_with(['.', ' '])
                || part.contains(['<', '>', '"', '|', '?', '*', '/'])
                || part.chars().any(char::is_control)
            {
                return Err(FsError::InvalidPath);
            }
            p.push(part);
            if let Ok(m) = fs::symlink_metadata(&p) {
                if is_link(&m) {
                    return Err(FsError::Denied);
                }
            }
        }
        Ok(p)
    }

    fn ensure_root(&self) -> Result<(), FsError> {
        fs::create_dir_all(&self.root).map_err(io_error)
    }
}

impl FileHost for FsFolder {
    fn open(&mut self, path: &str, mode: OpenMode) -> Result<FileHandle, FsError> {
        let p = self.host_path(path)?;
        if check_path(path)?.is_empty() || p.is_dir() {
            return Err(FsError::IsADirectory);
        }
        if mode.create || mode.create_new {
            self.ensure_root()?;
        }
        let file = OpenOptions::new()
            .read(mode.read || !mode.write)
            .write(mode.write)
            .create(mode.create && mode.write)
            .create_new(mode.create_new && mode.write)
            .truncate(mode.truncate && mode.write)
            .open(&p)
            .map_err(io_error)?;
        let h = self.next;
        self.next = self.next.wrapping_add(1).max(1);
        self.open.insert(
            h,
            Open {
                file,
                read: mode.read,
                write: mode.write,
            },
        );
        Ok(h)
    }

    fn read(&mut self, h: FileHandle, buf: &mut [u8]) -> Result<usize, FsError> {
        let o = self.open.get_mut(&h).ok_or(FsError::BadHandle)?;
        if !o.read {
            return Err(FsError::Denied);
        }
        let mut n = 0;
        while n < buf.len() {
            match o.file.read(&mut buf[n..]) {
                Ok(0) => break,
                Ok(k) => n += k,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return Err(io_error(e)),
            }
        }
        Ok(n)
    }

    fn write(&mut self, h: FileHandle, data: &[u8]) -> Result<usize, FsError> {
        let o = self.open.get_mut(&h).ok_or(FsError::BadHandle)?;
        if !o.write {
            return Err(FsError::Denied);
        }
        o.file.write_all(data).map_err(io_error)?;
        Ok(data.len())
    }

    fn seek(&mut self, h: FileHandle, pos: u64) -> Result<(), FsError> {
        let o = self.open.get_mut(&h).ok_or(FsError::BadHandle)?;
        o.file.seek(SeekFrom::Start(pos)).map_err(io_error)?;
        Ok(())
    }

    fn fstat(&mut self, h: FileHandle) -> Result<FileStat, FsError> {
        let o = self.open.get(&h).ok_or(FsError::BadHandle)?;
        o.file.metadata().map(|m| stat_of(&m)).map_err(io_error)
    }

    fn close(&mut self, h: FileHandle) {
        if let Some(mut o) = self.open.remove(&h) {
            let _ = o.file.flush();
        }
    }

    fn stat(&mut self, path: &str) -> Result<FileStat, FsError> {
        let p = self.host_path(path)?;
        if check_path(path)?.is_empty() && !p.exists() {
            // The root before its first write: an empty folder.
            return Ok(FileStat {
                is_dir: true,
                ..FileStat::default()
            });
        }
        fs::symlink_metadata(&p)
            .map(|m| stat_of(&m))
            .map_err(io_error)
    }

    fn list_dir(&mut self, path: &str) -> Result<Vec<DirEntry>, FsError> {
        let p = self.host_path(path)?;
        if check_path(path)?.is_empty() && !p.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for e in fs::read_dir(&p).map_err(io_error)? {
            let e = e.map_err(io_error)?;
            let Ok(m) = fs::symlink_metadata(e.path()) else {
                continue;
            };
            let Some(name) = e.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if is_link(&m) {
                continue;
            }
            out.push(DirEntry {
                name,
                stat: stat_of(&m),
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    fn mkdir(&mut self, path: &str) -> Result<(), FsError> {
        let p = self.host_path(path)?;
        self.ensure_root()?;
        fs::create_dir(p).map_err(io_error)
    }

    fn delete(&mut self, path: &str) -> Result<(), FsError> {
        if check_path(path)?.is_empty() {
            return Err(FsError::Denied);
        }
        let p = self.host_path(path)?;
        let m = fs::symlink_metadata(&p).map_err(io_error)?;
        if m.is_dir() {
            fs::remove_dir(&p).map_err(io_error)
        } else {
            fs::remove_file(&p).map_err(io_error)
        }
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<(), FsError> {
        if check_path(from)?.is_empty() || check_path(to)?.is_empty() {
            return Err(FsError::Denied);
        }
        let (a, b) = (self.host_path(from)?, self.host_path(to)?);
        if fs::symlink_metadata(&b).is_ok() {
            return Err(FsError::Exists);
        }
        fs::rename(a, b).map_err(io_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("mimic-hostfs-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        p
    }

    #[test]
    fn files_inside_the_root_only() {
        let root = temp_root("basic");
        let mut f = FsFolder::new(&root);
        assert_eq!(f.list_dir("/").unwrap(), []);
        let h = f
            .open(
                "/a.txt",
                OpenMode {
                    write: true,
                    read: true,
                    create: true,
                    ..Default::default()
                },
            )
            .unwrap();
        f.write(h, b"hello").unwrap();
        f.seek(h, 1).unwrap();
        let mut buf = [0u8; 10];
        assert_eq!(f.read(h, &mut buf).unwrap(), 4);
        assert_eq!(f.fstat(h).unwrap().size, 5);
        f.close(h);
        f.mkdir("/sub").unwrap();
        f.rename("/a.txt", "/sub/b.txt").unwrap();
        let names: Vec<String> = f
            .list_dir("/sub")
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, ["b.txt"]);
        assert_eq!(f.delete("/sub"), Err(FsError::NotEmpty));
        for bad in [
            "/../x",
            "/a/../../x",
            "c:/x",
            "/CON",
            "/nul.txt",
            "/x.",
            "/a\\b",
        ] {
            assert!(f.stat(bad).is_err(), "{bad}");
        }
        f.delete("/sub/b.txt").unwrap();
        f.delete("/sub").unwrap();
        assert_eq!(f.delete("/"), Err(FsError::Denied));
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_not_followed() {
        let root = temp_root("link");
        fs::create_dir_all(&root).unwrap();
        std::os::unix::fs::symlink("/etc", root.join("etc")).unwrap();
        let mut f = FsFolder::new(&root);
        assert_eq!(f.list_dir("/").unwrap(), []);
        assert_eq!(f.stat("/etc/passwd"), Err(FsError::Denied));
        let _ = fs::remove_dir_all(&root);
    }
}
