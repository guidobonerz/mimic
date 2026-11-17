//! A [`FileHost`] in memory: for tests, and for the web frontend, whose
//! files come from uploads instead of a host folder.

use std::collections::BTreeMap;

use super::{check_path, DirEntry, FileHandle, FileHost, FileStat, FsError, OpenMode};

#[derive(Clone, Debug, PartialEq, Eq)]
enum Node {
    Dir,
    File(Vec<u8>),
}

#[derive(Clone, Debug)]
struct Open {
    path: String,
    pos: usize,
    read: bool,
    write: bool,
}

/// An in-memory folder tree. Deterministic: no clock (`modified` is
/// `None`), names sorted.
#[derive(Clone, Debug)]
pub struct MemFileHost {
    nodes: BTreeMap<String, Node>,
    open: BTreeMap<FileHandle, Open>,
    next: FileHandle,
}

impl Default for MemFileHost {
    fn default() -> Self {
        Self::new()
    }
}

impl MemFileHost {
    /// An empty root folder.
    pub fn new() -> Self {
        let mut nodes = BTreeMap::new();
        nodes.insert("/".to_string(), Node::Dir);
        Self {
            nodes,
            open: BTreeMap::new(),
            next: 1,
        }
    }

    /// Put a file at canonical `path`, creating its directories.
    pub fn insert(&mut self, path: &str, data: Vec<u8>) -> Result<(), FsError> {
        let parts = check_path(path)?;
        if parts.is_empty() {
            return Err(FsError::IsADirectory);
        }
        let mut dir = String::new();
        for p in &parts[..parts.len() - 1] {
            dir.push('/');
            dir.push_str(p);
            match self.nodes.get(&dir) {
                Some(Node::File(_)) => return Err(FsError::NotADirectory),
                Some(Node::Dir) => {}
                None => {
                    self.nodes.insert(dir.clone(), Node::Dir);
                }
            }
        }
        if self.nodes.get(path) == Some(&Node::Dir) {
            return Err(FsError::IsADirectory);
        }
        self.nodes.insert(path.to_string(), Node::File(data));
        Ok(())
    }

    /// The contents of the file at `path`.
    pub fn get(&self, path: &str) -> Option<&[u8]> {
        match self.nodes.get(path) {
            Some(Node::File(d)) => Some(d),
            _ => None,
        }
    }

    fn parent(path: &str) -> &str {
        match path.rfind('/') {
            Some(0) | None => "/",
            Some(i) => &path[..i],
        }
    }

    fn need_parent_dir(&self, path: &str) -> Result<(), FsError> {
        match self.nodes.get(Self::parent(path)) {
            Some(Node::Dir) => Ok(()),
            Some(Node::File(_)) => Err(FsError::NotADirectory),
            None => Err(FsError::NotFound),
        }
    }

    fn stat_of(node: &Node) -> FileStat {
        match node {
            Node::Dir => FileStat {
                is_dir: true,
                ..FileStat::default()
            },
            Node::File(d) => FileStat {
                size: d.len() as u64,
                ..FileStat::default()
            },
        }
    }

    fn children<'a>(&'a self, dir: &'a str) -> impl Iterator<Item = (&'a String, &'a Node)> + 'a {
        let prefix = if dir == "/" {
            "/".to_string()
        } else {
            format!("{dir}/")
        };
        self.nodes
            .range(prefix.clone()..)
            .take_while(move |(k, _)| k.starts_with(&prefix))
            .filter(move |(k, _)| {
                let rest = &k[if dir == "/" { 1 } else { dir.len() + 1 }..];
                !rest.is_empty() && !rest.contains('/')
            })
    }
}

impl FileHost for MemFileHost {
    fn open(&mut self, path: &str, mode: OpenMode) -> Result<FileHandle, FsError> {
        check_path(path)?;
        match self.nodes.get(path) {
            Some(Node::Dir) => return Err(FsError::IsADirectory),
            Some(Node::File(_)) if mode.create_new => return Err(FsError::Exists),
            Some(Node::File(_)) => {}
            None if mode.create || mode.create_new => {
                self.need_parent_dir(path)?;
                self.nodes.insert(path.to_string(), Node::File(Vec::new()));
            }
            None => return Err(FsError::NotFound),
        }
        if mode.truncate && mode.write {
            self.nodes.insert(path.to_string(), Node::File(Vec::new()));
        }
        let h = self.next;
        self.next = self.next.wrapping_add(1).max(1);
        self.open.insert(
            h,
            Open {
                path: path.to_string(),
                pos: 0,
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
        let Some(Node::File(d)) = self.nodes.get(&o.path) else {
            return Err(FsError::BadHandle);
        };
        let n = d.len().saturating_sub(o.pos).min(buf.len());
        buf[..n].copy_from_slice(&d[o.pos..o.pos + n]);
        o.pos += n;
        Ok(n)
    }

    fn write(&mut self, h: FileHandle, data: &[u8]) -> Result<usize, FsError> {
        let o = self.open.get_mut(&h).ok_or(FsError::BadHandle)?;
        if !o.write {
            return Err(FsError::Denied);
        }
        let Some(Node::File(d)) = self.nodes.get_mut(&o.path) else {
            return Err(FsError::BadHandle);
        };
        let end = o.pos + data.len();
        if d.len() < end {
            d.resize(end, 0);
        }
        d[o.pos..end].copy_from_slice(data);
        o.pos = end;
        Ok(data.len())
    }

    fn seek(&mut self, h: FileHandle, pos: u64) -> Result<(), FsError> {
        let o = self.open.get_mut(&h).ok_or(FsError::BadHandle)?;
        o.pos = usize::try_from(pos).map_err(|_| FsError::InvalidPath)?;
        Ok(())
    }

    fn fstat(&mut self, h: FileHandle) -> Result<FileStat, FsError> {
        let o = self.open.get(&h).ok_or(FsError::BadHandle)?;
        self.nodes
            .get(&o.path)
            .map(Self::stat_of)
            .ok_or(FsError::BadHandle)
    }

    fn close(&mut self, h: FileHandle) {
        self.open.remove(&h);
    }

    fn stat(&mut self, path: &str) -> Result<FileStat, FsError> {
        check_path(path)?;
        self.nodes
            .get(path)
            .map(Self::stat_of)
            .ok_or(FsError::NotFound)
    }

    fn list_dir(&mut self, path: &str) -> Result<Vec<DirEntry>, FsError> {
        check_path(path)?;
        match self.nodes.get(path) {
            Some(Node::Dir) => {}
            Some(Node::File(_)) => return Err(FsError::NotADirectory),
            None => return Err(FsError::NotFound),
        }
        Ok(self
            .children(path)
            .map(|(k, n)| DirEntry {
                name: k[k.rfind('/').map_or(0, |i| i + 1)..].to_string(),
                stat: Self::stat_of(n),
            })
            .collect())
    }

    fn mkdir(&mut self, path: &str) -> Result<(), FsError> {
        if check_path(path)?.is_empty() || self.nodes.contains_key(path) {
            return Err(FsError::Exists);
        }
        self.need_parent_dir(path)?;
        self.nodes.insert(path.to_string(), Node::Dir);
        Ok(())
    }

    fn delete(&mut self, path: &str) -> Result<(), FsError> {
        if check_path(path)?.is_empty() {
            return Err(FsError::Denied);
        }
        match self.nodes.get(path) {
            None => Err(FsError::NotFound),
            Some(Node::Dir) if self.children(path).next().is_some() => Err(FsError::NotEmpty),
            Some(_) => {
                self.nodes.remove(path);
                Ok(())
            }
        }
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<(), FsError> {
        if check_path(from)?.is_empty() || check_path(to)?.is_empty() {
            return Err(FsError::Denied);
        }
        if self.nodes.contains_key(to) {
            return Err(FsError::Exists);
        }
        self.need_parent_dir(to)?;
        let node = self.nodes.remove(from).ok_or(FsError::NotFound)?;
        if node == Node::Dir {
            // Move the subtree along.
            let prefix = format!("{from}/");
            let inner: Vec<String> = self
                .nodes
                .range(prefix.clone()..)
                .take_while(|(k, _)| k.starts_with(&prefix))
                .map(|(k, _)| k.clone())
                .collect();
            for k in inner {
                if let Some(n) = self.nodes.remove(&k) {
                    self.nodes.insert(format!("{to}/{}", &k[prefix.len()..]), n);
                }
            }
        }
        for o in self.open.values_mut() {
            if o.path == from {
                o.path = to.to_string();
            }
        }
        self.nodes.insert(to.to_string(), node);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_and_directories() {
        let mut fs = MemFileHost::new();
        fs.insert("/games/a.prg", vec![1, 2, 3]).unwrap();
        fs.mkdir("/docs").unwrap();
        let names: Vec<String> = fs
            .list_dir("/")
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, ["docs", "games"]);
        assert_eq!(fs.list_dir("/games").unwrap()[0].stat.size, 3);
        assert_eq!(fs.delete("/games"), Err(FsError::NotEmpty));
        fs.rename("/games", "/progs").unwrap();
        assert_eq!(fs.get("/progs/a.prg"), Some(&[1u8, 2, 3][..]));
        let h = fs
            .open(
                "/docs/new.txt",
                OpenMode {
                    write: true,
                    create_new: true,
                    ..Default::default()
                },
            )
            .unwrap();
        fs.write(h, b"hi").unwrap();
        let mut buf = [0; 2];
        assert_eq!(fs.read(h, &mut buf), Err(FsError::Denied));
        fs.close(h);
        assert_eq!(fs.stat("/docs/new.txt").unwrap().size, 2);
        assert_eq!(
            fs.open("/x/../y", OpenMode::default()),
            Err(FsError::InvalidPath)
        );
        assert_eq!(
            fs.open("/nope", OpenMode::default()),
            Err(FsError::NotFound)
        );
        fs.delete("/docs/new.txt").unwrap();
        fs.delete("/docs").unwrap();
    }
}
