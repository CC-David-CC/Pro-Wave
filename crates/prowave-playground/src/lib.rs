// SPDX-License-Identifier: GPL-3.0-or-later
//! Pro Wave in the browser, for Playground (patches/logic-pro.yaml in github.com/daijro/openplayground).
//!
//! wasm32-unknown-unknown has no clock, no files, no threads and no process: `Instant::now` and `SystemTime::now`
//! panic there, every file call fails, `thread::spawn` panics, and so do `env::temp_dir` and `process::id`. The modules
//! that use them take `std` from here instead, through one `#[cfg(target_arch = "wasm32")] use prowave_playground::std;`
//! line at each one's end (a local `std` shadows the standard library's name in that module; everything else in it
//! still comes from the real std):
//!   time     web-time's Instant and SystemTime;
//!   fs       an in-memory file system (projects, their media, settings), each change noted for the page to mirror;
//!   thread   `spawn` queues what it is given for the page to run after the frame, on this thread;
//!   env      `temp_dir` is `/tmp`, and `HOME` is `/home` (settings live in memory, so every page starts afresh);
//!   process  `id` is 1.
#![cfg(target_arch = "wasm32")]

pub mod std {
    pub use ::std::*;

    pub mod time {
        pub use web_time::*;
    }

    pub mod process {
        pub use ::std::process::*;

        /// The page is one process (std's panics on wasm).
        pub fn id() -> u32 {
            1
        }
    }

    pub mod env {
        pub use ::std::env::*;
        use ::std::ffi::{OsStr, OsString};
        use ::std::path::PathBuf;

        /// A folder in the in-memory files (std's panics on wasm).
        pub fn temp_dir() -> PathBuf {
            PathBuf::from("/tmp")
        }

        /// `HOME` is `/home`: settings go to the in-memory files. Nothing else is set.
        pub fn var_os<K: AsRef<OsStr>>(key: K) -> Option<OsString> {
            (key.as_ref() == "HOME").then(|| OsString::from("/home"))
        }
    }

    pub mod thread {
        pub use ::std::thread::*;
        use ::std::cell::RefCell;

        thread_local! {
            static QUEUED: RefCell<Vec<Box<dyn FnOnce()>>> = RefCell::default();
        }

        /// Queues `f` for [`run_spawned`] (a page has one thread, and the frame should not wait for it).
        pub fn spawn<F, T>(f: F) -> Queued
        where
            F: FnOnce() -> T + Send + 'static,
            T: Send + 'static,
        {
            QUEUED.with_borrow_mut(|q| q.push(Box::new(move || drop(f()))));
            Queued
        }

        /// What [`spawn`] gives back: the work waits for [`run_spawned`].
        pub struct Queued;

        /// Runs what was spawned since the last call (the page calls it after each frame).
        pub fn run_spawned() {
            for job in QUEUED.take() {
                job();
            }
        }

        /// Drops what was spawned since the last [`run_spawned`], unrun.
        pub fn forget_spawned() {
            QUEUED.take();
        }
    }

    pub mod fs {
        pub use ::std::fs::*;
        use ::std::cell::RefCell;
        use ::std::collections::{BTreeMap, BTreeSet};
        use ::std::io::{self, Cursor, Read, Seek, SeekFrom, Write};
        use ::std::path::{Component, Path, PathBuf};

        enum Entry {
            Dir,
            File(Vec<u8>, web_time::SystemTime),
        }

        #[derive(Default)]
        struct Tree {
            entries: BTreeMap<PathBuf, Entry>,
            /// Paths written, created or removed since the last [`changes`].
            changed: BTreeSet<PathBuf>,
        }

        thread_local! {
            static TREE: RefCell<Tree> = RefCell::default();
        }

        fn with<T>(f: impl FnOnce(&mut Tree) -> T) -> T {
            TREE.with(|t| f(&mut t.borrow_mut()))
        }

        /// `/a/./b/../c` -> `/a/c`: one spelling per file (relative paths start at `/`).
        pub fn key(p: &Path) -> PathBuf {
            let mut out = PathBuf::from("/");
            for c in p.components() {
                match c {
                    Component::ParentDir => {
                        out.pop();
                    }
                    Component::Normal(n) => out.push(n),
                    _ => {}
                }
            }
            out
        }

        fn missing(p: &Path) -> io::Error {
            io::Error::new(io::ErrorKind::NotFound, format!("{} does not exist", p.display()))
        }

        impl Tree {
            fn mark(&mut self, p: PathBuf) {
                self.changed.insert(p);
            }
            fn parents(&mut self, p: &Path) {
                for dir in p.ancestors().skip(1) {
                    self.entries.entry(dir.to_path_buf()).or_insert(Entry::Dir);
                }
            }
            fn put(&mut self, p: PathBuf, bytes: Vec<u8>) {
                self.parents(&p);
                self.entries.insert(p, Entry::File(bytes, web_time::SystemTime::now()));
            }
            /// `p` and everything below it.
            fn below(&self, p: &Path) -> Vec<PathBuf> {
                self.entries.range(p.to_path_buf()..).map(|(k, _)| k).take_while(|k| k.starts_with(p)).cloned().collect()
            }
        }

        /// Stores a file without noting a change (files the page brings in from its storage or a session).
        pub fn put(p: &Path, bytes: Vec<u8>) {
            with(|t| t.put(key(p), bytes));
        }

        /// The paths changed since the last call: each now holds what the page's storage should hold, or is gone.
        pub fn changes() -> Vec<PathBuf> {
            with(|t| ::std::mem::take(&mut t.changed).into_iter().collect())
        }

        pub fn exists<P: AsRef<Path>>(p: P) -> io::Result<bool> {
            Ok(metadata(p).is_ok())
        }

        pub fn write<P: AsRef<Path>, C: AsRef<[u8]>>(p: P, bytes: C) -> io::Result<()> {
            let p = key(p.as_ref());
            with(|t| {
                t.put(p.clone(), bytes.as_ref().to_vec());
                t.mark(p);
            });
            Ok(())
        }

        pub fn read<P: AsRef<Path>>(p: P) -> io::Result<Vec<u8>> {
            TREE.with(|t| match t.borrow().entries.get(&key(p.as_ref())) {
                Some(Entry::File(bytes, _)) => Ok(bytes.clone()),
                _ => Err(missing(p.as_ref())),
            })
        }

        pub fn read_to_string<P: AsRef<Path>>(p: P) -> io::Result<String> {
            String::from_utf8(read(p)?).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
        }

        pub fn create_dir_all<P: AsRef<Path>>(p: P) -> io::Result<()> {
            let p = key(p.as_ref());
            with(|t| {
                if !t.entries.contains_key(&p) {
                    t.parents(&p);
                    t.entries.insert(p.clone(), Entry::Dir);
                    t.mark(p);
                }
            });
            Ok(())
        }

        pub fn create_dir<P: AsRef<Path>>(p: P) -> io::Result<()> {
            if metadata(p.as_ref()).is_ok() {
                return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{} exists", p.as_ref().display())));
            }
            create_dir_all(p)
        }

        pub fn remove_file<P: AsRef<Path>>(p: P) -> io::Result<()> {
            let p = key(p.as_ref());
            with(|t| match t.entries.get(&p) {
                Some(Entry::File(..)) => {
                    t.entries.remove(&p);
                    t.mark(p);
                    Ok(())
                }
                _ => Err(missing(&p)),
            })
        }

        pub fn remove_dir_all<P: AsRef<Path>>(p: P) -> io::Result<()> {
            let p = key(p.as_ref());
            with(|t| {
                let gone = t.below(&p);
                if gone.is_empty() {
                    return Err(missing(&p));
                }
                for g in gone {
                    t.entries.remove(&g);
                    t.mark(g);
                }
                Ok(())
            })
        }

        pub fn rename<P: AsRef<Path>, Q: AsRef<Path>>(from: P, to: Q) -> io::Result<()> {
            let (from, to) = (key(from.as_ref()), key(to.as_ref()));
            let bytes = read(&from)?;
            with(|t| {
                t.entries.remove(&from);
                t.mark(from);
                t.put(to.clone(), bytes);
                t.mark(to);
            });
            Ok(())
        }

        pub fn copy<P: AsRef<Path>, Q: AsRef<Path>>(from: P, to: Q) -> io::Result<u64> {
            let bytes = read(from)?;
            let n = bytes.len() as u64;
            write(to, bytes)?;
            Ok(n)
        }

        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub struct FileType(bool);
        impl FileType {
            pub fn is_file(&self) -> bool {
                self.0
            }
            pub fn is_dir(&self) -> bool {
                !self.0
            }
            pub fn is_symlink(&self) -> bool {
                false
            }
        }

        #[derive(Clone, Debug)]
        pub struct Metadata(Option<(u64, web_time::SystemTime)>);
        impl Metadata {
            pub fn len(&self) -> u64 {
                self.0.map_or(0, |(n, _)| n)
            }
            pub fn is_file(&self) -> bool {
                self.0.is_some()
            }
            pub fn is_dir(&self) -> bool {
                self.0.is_none()
            }
            pub fn is_symlink(&self) -> bool {
                false
            }
            pub fn file_type(&self) -> FileType {
                FileType(self.is_file())
            }
            pub fn modified(&self) -> io::Result<web_time::SystemTime> {
                Ok(self.0.map_or(web_time::SystemTime::UNIX_EPOCH, |(_, t)| t))
            }
        }

        pub fn metadata<P: AsRef<Path>>(p: P) -> io::Result<Metadata> {
            TREE.with(|t| match t.borrow().entries.get(&key(p.as_ref())) {
                Some(Entry::File(bytes, at)) => Ok(Metadata(Some((bytes.len() as u64, *at)))),
                Some(Entry::Dir) => Ok(Metadata(None)),
                None => Err(missing(p.as_ref())),
            })
        }

        pub struct DirEntry(PathBuf);
        impl DirEntry {
            pub fn path(&self) -> PathBuf {
                self.0.clone()
            }
            pub fn file_name(&self) -> ::std::ffi::OsString {
                self.0.file_name().unwrap_or_default().to_os_string()
            }
            pub fn metadata(&self) -> io::Result<Metadata> {
                metadata(&self.0)
            }
            pub fn file_type(&self) -> io::Result<FileType> {
                Ok(metadata(&self.0)?.file_type())
            }
        }

        pub type ReadDir = ::std::vec::IntoIter<io::Result<DirEntry>>;

        pub fn read_dir<P: AsRef<Path>>(p: P) -> io::Result<ReadDir> {
            let dir = key(p.as_ref());
            if !metadata(&dir)?.is_dir() {
                return Err(io::Error::other(format!("{} is not a folder", dir.display())));
            }
            let entries = TREE.with(|t| {
                t.borrow().below(&dir).into_iter().filter(|k| k.parent() == Some(&dir)).map(|k| Ok(DirEntry(k))).collect::<Vec<_>>()
            });
            Ok(entries.into_iter())
        }

        /// A file's bytes, read whole when it opens; one opened for writing is stored back when flushed or dropped.
        pub struct File {
            data: Cursor<Vec<u8>>,
            /// Where written bytes go, for a file opened to write.
            to: Option<PathBuf>,
            dirty: bool,
        }

        impl File {
            pub fn open<P: AsRef<Path>>(p: P) -> io::Result<File> {
                Ok(File { data: Cursor::new(read(p)?), to: None, dirty: false })
            }
            pub fn create<P: AsRef<Path>>(p: P) -> io::Result<File> {
                write(p.as_ref(), [])?;
                Ok(File { data: Cursor::new(Vec::new()), to: Some(key(p.as_ref())), dirty: false })
            }
            pub fn metadata(&self) -> io::Result<Metadata> {
                Ok(Metadata(Some((self.data.get_ref().len() as u64, web_time::SystemTime::now()))))
            }
            pub fn sync_all(&self) -> io::Result<()> {
                Ok(())
            }
            fn store(&mut self) {
                if let (true, Some(to)) = (self.dirty, &self.to) {
                    let _ = write(to, self.data.get_ref());
                    self.dirty = false;
                }
            }
        }

        impl Read for File {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                self.data.read(buf)
            }
        }

        impl Seek for File {
            fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
                self.data.seek(pos)
            }
        }

        impl Write for File {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                if self.to.is_none() {
                    return Err(io::Error::new(io::ErrorKind::PermissionDenied, "the file is open to read"));
                }
                self.dirty = true;
                self.data.write(buf)
            }
            fn flush(&mut self) -> io::Result<()> {
                self.store();
                Ok(())
            }
        }

        impl symphonia_core::io::MediaSource for File {
            fn is_seekable(&self) -> bool {
                true
            }
            fn byte_len(&self) -> Option<u64> {
                Some(self.data.get_ref().len() as u64)
            }
        }

        impl Drop for File {
            fn drop(&mut self) {
                self.store();
            }
        }
    }
}
