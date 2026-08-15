use std::ffi::CString;
use std::fs::File;
use std::io;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use super::path::RelPath;

const RESOLVE_NO_MAGICLINKS: u64 = 0x02;
const RESOLVE_NO_SYMLINKS: u64 = 0x04;
const RESOLVE_BENEATH: u64 = 0x08;

const INSIDE: u64 = RESOLVE_BENEATH | RESOLVE_NO_MAGICLINKS;
const ON_THE_WAY_IN: u64 = INSIDE | RESOLVE_NO_SYMLINKS;

pub const FILE_MODE: u32 = 0o660;
pub const DIR_MODE: u32 = 0o770;

const MAX_WALK_DEPTH: usize = 256;

#[repr(C)]
struct OpenHow {
    flags: u64,
    mode: u64,
    resolve: u64,
}

#[derive(Debug)]
pub struct Root {
    fd: OwnedFd,
    path: PathBuf,
}

#[derive(Debug)]
pub struct Dir {
    fd: OwnedFd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Meta {
    pub kind: Kind,
    pub size: u64,
    pub modified: i64,
    pub created: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Directory,
    Symlink,
    Other,
}

impl Kind {
    pub const fn on_the_wire(self) -> &'static str {
        match self {
            Self::Directory => "directory",
            Self::Symlink => "symlink",
            _ => "file",
        }
    }
}

impl Root {
    pub fn open(path: impl Into<PathBuf>) -> io::Result<Self> {
        let path = path.into();
        let name = cstring(std::os::unix::ffi::OsStrExt::as_bytes(path.as_os_str()))?;
        let raw = unsafe {
            libc::open(name.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC)
        };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { fd: unsafe { OwnedFd::from_raw_fd(raw) }, path })
    }

    pub fn open_beneath(base: impl Into<PathBuf>, steps: &[String]) -> io::Result<Self> {
        let base = Self::open(base)?;
        let mut path = base.path;
        for step in steps {
            path.push(step);
        }

        let fd = if openat2_works() {
            match openat2(base.fd.as_fd(), &steps.join("/"), DIRECTORY, ON_THE_WAY_IN) {
                Err(err) if err.raw_os_error() == Some(libc::ENOSYS) => {
                    openat2_gone();
                    step_in(base.fd.as_fd(), steps)?
                }
                other => other?,
            }
        } else {
            step_in(base.fd.as_fd(), steps)?
        };
        Ok(Self { fd, path })
    }

    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self { fd: self.fd.try_clone()?, path: self.path.clone() })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn full_path(&self, rel: &RelPath) -> PathBuf {
        let mut path = self.path.clone();
        for segment in rel.segments() {
            path.push(segment);
        }
        path
    }

    fn resolve(&self, rel: &RelPath, flags: i32) -> io::Result<OwnedFd> {
        if openat2_works() {
            match openat2(self.fd.as_fd(), &rel.beneath_root(), flags, INSIDE) {
                Err(err) if err.raw_os_error() == Some(libc::ENOSYS) => openat2_gone(),
                other => return other,
            }
        }
        self.walk(rel, flags)
    }

    fn walk(&self, rel: &RelPath, flags: i32) -> io::Result<OwnedFd> {
        let mut here = self.fd.try_clone()?;
        let (last, leading) = match rel.segments().split_last() {
            None => return Ok(here),
            Some((last, leading)) => (last, leading),
        };
        for segment in leading {
            let step = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
            here = openat(here.as_fd(), segment.as_bytes(), step, 0)?;
        }
        openat(here.as_fd(), last.as_bytes(), flags | libc::O_NOFOLLOW, 0)
    }

    pub fn dir(&self, rel: &RelPath) -> io::Result<Dir> {
        let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC;
        Ok(Dir { fd: self.resolve(rel, flags)? })
    }

    pub fn parent_of(&self, rel: &RelPath) -> io::Result<Dir> {
        self.dir(&rel.parent())
    }

    pub fn open_read(&self, rel: &RelPath) -> io::Result<File> {
        let flags = libc::O_RDONLY | libc::O_NONBLOCK | libc::O_CLOEXEC;
        Ok(File::from(self.resolve(rel, flags)?))
    }

    pub fn meta(&self, rel: &RelPath) -> io::Result<Meta> {
        match rel.name() {
            None => Ok(Meta { kind: Kind::Directory, size: 0, modified: 0, created: 0 }),
            Some(name) => self.parent_of(rel)?.meta(name.as_bytes()),
        }
    }

    pub fn exists(&self, rel: &RelPath) -> bool {
        self.meta(rel).is_ok()
    }
}

impl Dir {
    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self { fd: self.fd.try_clone()? })
    }

    pub fn meta(&self, name: &[u8]) -> io::Result<Meta> {
        let flags = libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC;
        let entry = File::from(openat(self.fd.as_fd(), name, flags, 0)?);
        let meta = entry.metadata()?;

        let kind = if meta.is_dir() {
            Kind::Directory
        } else if meta.is_symlink() {
            Kind::Symlink
        } else if meta.is_file() {
            Kind::File
        } else {
            Kind::Other
        };

        Ok(Meta {
            kind,
            size: meta.len(),
            modified: unix_seconds(meta.modified()),
            created: unix_seconds(meta.created()),
        })
    }

    pub fn entries(&self) -> io::Result<Vec<Vec<u8>>> {
        read_dir(self.fd.as_fd())
    }

    pub fn count(&self) -> io::Result<u64> {
        let mut seen = 0;
        walk_dir(self.fd.as_fd(), |_| seen += 1)?;
        Ok(seen)
    }

    pub fn child(&self, name: &[u8]) -> io::Result<Self> {
        let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
        Ok(Self { fd: openat(self.fd.as_fd(), name, flags, 0)? })
    }

    pub fn create_file(&self, name: &[u8]) -> io::Result<File> {
        let flags = libc::O_WRONLY
            | libc::O_CREAT
            | libc::O_EXCL
            | libc::O_NOFOLLOW
            | libc::O_CLOEXEC;
        Ok(File::from(openat(self.fd.as_fd(), name, flags, FILE_MODE)?))
    }

    pub fn create_dir(&self, name: &[u8]) -> io::Result<()> {
        let name = cstring(name)?;
        check(unsafe { libc::mkdirat(self.fd.as_raw_fd(), name.as_ptr(), DIR_MODE) })
    }

    pub fn ensure_dir(&self, name: &[u8]) -> io::Result<()> {
        match self.create_dir(name) {
            Err(err) if err.raw_os_error() == Some(libc::EEXIST) => {
                match self.meta(name)?.kind {
                    Kind::Directory => Ok(()),
                    _ => Err(io::Error::from_raw_os_error(libc::ENOTDIR)),
                }
            }
            other => other,
        }
    }

    pub fn unlink(&self, name: &[u8]) -> io::Result<()> {
        let name = cstring(name)?;
        check(unsafe { libc::unlinkat(self.fd.as_raw_fd(), name.as_ptr(), 0) })
    }

    pub fn rmdir(&self, name: &[u8]) -> io::Result<()> {
        let name = cstring(name)?;
        check(unsafe {
            libc::unlinkat(self.fd.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR)
        })
    }

    pub fn read_link(&self, name: &[u8]) -> io::Result<String> {
        let name = cstring(name)?;
        let mut buffer = vec![0u8; super::path::MAX_PATH_BYTES];
        let written = unsafe {
            libc::readlinkat(
                self.fd.as_raw_fd(),
                name.as_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
            )
        };
        if written < 0 {
            return Err(io::Error::last_os_error());
        }
        buffer.truncate(written as usize);
        Ok(String::from_utf8_lossy(&buffer).into_owned())
    }

    pub fn rename_to(
        &self,
        name: &[u8],
        target: &Self,
        target_name: &[u8],
        replace: bool,
    ) -> io::Result<()> {
        let from = cstring(name)?;
        let to = cstring(target_name)?;

        if !replace {
            let done = unsafe {
                libc::syscall(
                    libc::SYS_renameat2,
                    self.fd.as_raw_fd(),
                    from.as_ptr(),
                    target.fd.as_raw_fd(),
                    to.as_ptr(),
                    libc::RENAME_NOREPLACE,
                )
            };
            if done == 0 {
                return Ok(());
            }
            let err = io::Error::last_os_error();
            match err.raw_os_error() {
                Some(libc::ENOSYS) | Some(libc::EINVAL) | Some(libc::ENOTTY) => {
                    if target.meta(target_name).is_ok() {
                        return Err(io::Error::from_raw_os_error(libc::EEXIST));
                    }
                }
                _ => return Err(err),
            }
        }

        check(unsafe {
            libc::renameat(self.fd.as_raw_fd(), from.as_ptr(), target.fd.as_raw_fd(), to.as_ptr())
        })
    }

    pub fn only_lossy_match(&self, lossy: &str) -> io::Result<Option<Vec<u8>>> {
        let mut found = None;
        for name in self.entries()? {
            if std::str::from_utf8(&name).is_ok() {
                continue;
            }
            if String::from_utf8_lossy(&name) == lossy {
                if found.is_some() {
                    return Ok(None);
                }
                found = Some(name);
            }
        }
        Ok(found)
    }

    pub fn remove_tree(&self, name: &[u8]) -> io::Result<()> {
        self.remove_tree_at(name, 0)
    }

    fn remove_tree_at(&self, name: &[u8], depth: usize) -> io::Result<()> {
        match self.unlink(name) {
            Ok(()) => return Ok(()),
            Err(err) => match err.raw_os_error() {
                Some(libc::EISDIR) | Some(libc::EPERM) => {}
                _ => return Err(err),
            },
        }
        if depth >= MAX_WALK_DEPTH {
            return Err(io::Error::from_raw_os_error(libc::ENAMETOOLONG));
        }

        let child = self.child(name)?;
        for entry in child.entries()? {
            child.remove_tree_at(&entry, depth + 1)?;
        }
        drop(child);
        self.rmdir(name)
    }

    pub fn copy_tree(&self, name: &[u8], target: &Self, target_name: &[u8]) -> io::Result<()> {
        self.copy_tree_at(name, target, target_name, 0)
    }

    fn copy_tree_at(
        &self,
        name: &[u8],
        target: &Self,
        target_name: &[u8],
        depth: usize,
    ) -> io::Result<()> {
        if depth >= MAX_WALK_DEPTH {
            return Err(io::Error::from_raw_os_error(libc::ENAMETOOLONG));
        }
        match self.meta(name)?.kind {
            Kind::Directory => {
                target.ensure_dir(target_name)?;
                let from = self.child(name)?;
                let into = target.child(target_name)?;
                for entry in from.entries()? {
                    from.copy_tree_at(&entry, &into, &entry, depth + 1)?;
                }
                Ok(())
            }
            Kind::File => {
                let flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
                let mut source = File::from(openat(self.fd.as_fd(), name, flags, 0)?);
                let mut sink = target.create_file(target_name)?;
                io::copy(&mut source, &mut sink)?;
                sink.sync_all()
            }
            Kind::Symlink => {
                let points_at = cstring(self.read_link(name)?.as_bytes())?;
                let here = cstring(target_name)?;
                check(unsafe {
                    libc::symlinkat(points_at.as_ptr(), target.fd.as_raw_fd(), here.as_ptr())
                })
            }
            Kind::Other => Ok(()),
        }
    }
}

impl AsFd for Dir {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

pub struct Part {
    dir: Dir,
    name: Vec<u8>,
    committed: bool,
}

impl Part {
    pub fn create(dir: Dir, target: &str) -> io::Result<(Self, File)> {
        let mut cut = target.len().min(160);
        while cut > 0 && !target.is_char_boundary(cut) {
            cut -= 1;
        }
        let name = format!(".{}.part.{}", &target[..cut], crate::model::Id::new()).into_bytes();
        let file = dir.create_file(&name)?;
        Ok((Self { dir, name, committed: false }, file))
    }

    pub fn name(&self) -> &[u8] {
        &self.name
    }

    pub fn commit(mut self, target: &[u8], replace: bool) -> io::Result<()> {
        let outcome = self.dir.rename_to(&self.name, &self.dir, target, replace);
        self.committed = outcome.is_ok();
        outcome
    }
}

impl Drop for Part {
    fn drop(&mut self) {
        if !self.committed {
            let _ = self.dir.unlink(&self.name);
        }
    }
}

pub fn seconds_of(time: io::Result<SystemTime>) -> i64 {
    unix_seconds(time)
}

fn unix_seconds(time: io::Result<SystemTime>) -> i64 {
    time.ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_secs() as i64)
}

fn cstring(bytes: &[u8]) -> io::Result<CString> {
    CString::new(bytes).map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))
}

fn check(outcome: libc::c_int) -> io::Result<()> {
    if outcome < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn openat(dir: BorrowedFd<'_>, name: &[u8], flags: i32, mode: u32) -> io::Result<OwnedFd> {
    let name = cstring(name)?;
    let raw = unsafe { libc::openat(dir.as_raw_fd(), name.as_ptr(), flags, mode as libc::c_uint) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(raw) })
}

const DIRECTORY: i32 = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC;

fn step_in(base: BorrowedFd<'_>, steps: &[String]) -> io::Result<OwnedFd> {
    let mut here = base.try_clone_to_owned()?;
    for step in steps {
        here = openat(here.as_fd(), step.as_bytes(), DIRECTORY | libc::O_NOFOLLOW, 0)?;
    }
    Ok(here)
}

fn openat2(dir: BorrowedFd<'_>, path: &str, flags: i32, resolve: u64) -> io::Result<OwnedFd> {
    let path = cstring(path.as_bytes())?;
    let how = OpenHow { flags: (flags | libc::O_CLOEXEC) as u32 as u64, mode: 0, resolve };
    let raw = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            dir.as_raw_fd(),
            path.as_ptr(),
            std::ptr::addr_of!(how),
            std::mem::size_of::<OpenHow>(),
        )
    };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(raw as RawFd) })
}

static OPENAT2: AtomicBool = AtomicBool::new(true);

fn openat2_works() -> bool {
    OPENAT2.load(Ordering::Relaxed)
}

fn openat2_gone() {
    if OPENAT2.swap(false, Ordering::Relaxed) {
        tracing::warn!("this kernel has no openat2; falling back to one openat per segment");
    }
}

#[repr(C)]
struct Dirent {
    inode: u64,
    offset: i64,
    length: u16,
    kind: u8,
}

fn read_dir(fd: BorrowedFd<'_>) -> io::Result<Vec<Vec<u8>>> {
    let mut names = Vec::new();
    walk_dir(fd, |name| names.push(name.to_vec()))?;
    Ok(names)
}

fn walk_dir(fd: BorrowedFd<'_>, mut each: impl FnMut(&[u8])) -> io::Result<()> {
    let mut buffer = vec![0u64; 4096];
    let bytes = buffer.len() * std::mem::size_of::<u64>();

    if unsafe { libc::lseek(fd.as_raw_fd(), 0, libc::SEEK_SET) } < 0 {
        return Err(io::Error::last_os_error());
    }

    loop {
        let read = unsafe {
            libc::syscall(libc::SYS_getdents64, fd.as_raw_fd(), buffer.as_mut_ptr(), bytes)
        };
        if read < 0 {
            return Err(io::Error::last_os_error());
        }
        if read == 0 {
            return Ok(());
        }

        let mut at = 0usize;
        let base = buffer.as_ptr().cast::<u8>();
        while at < read as usize {
            let record = unsafe { base.add(at) };
            let header = unsafe { std::ptr::read_unaligned(record.cast::<Dirent>()) };
            let length = header.length as usize;
            let name_at = std::mem::offset_of!(Dirent, kind) + 1;
            if length <= name_at || at + length > read as usize {
                return Err(io::Error::from_raw_os_error(libc::EIO));
            }

            let name = unsafe {
                let start = record.add(name_at);
                let room = length - name_at;
                let bytes = std::slice::from_raw_parts(start, room);
                bytes.split(|byte| *byte == 0).next().unwrap_or_default()
            };
            if name != b"." && name != b".." {
                each(name);
            }
            at += length;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::path::RelPath;
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::fs::symlink;

    struct Tree(PathBuf);

    impl Tree {
        fn new() -> Self {
            let path = std::env::temp_dir()
                .join(format!("craftpanel-jail-{}-{}", std::process::id(), crate::model::Id::new()));
            std::fs::create_dir_all(path.join("plugins/config")).expect("a tree");
            std::fs::write(path.join("server.properties"), b"level-name=world\n").expect("a file");
            std::fs::write(path.join("plugins/config/one.yml"), b"a: 1\n").expect("a file");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn root(&self) -> Root {
            Root::open(&self.0).expect("the root opens")
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    fn at(raw: &str) -> RelPath {
        RelPath::parse(raw).expect("a usable path")
    }

    #[test]
    fn a_link_that_points_out_of_the_tree_is_refused_by_the_kernel() {
        let tree = Tree::new();
        let secret = tree.path().parent().expect("a parent").join("panel.db");
        std::fs::write(&secret, b"password hashes").expect("the database");
        symlink(&secret, tree.path().join("logs.link")).expect("the link");

        let root = tree.root();
        let refused = root.open_read(&at("/logs.link")).expect_err("this must not open");
        assert_eq!(
            refused.raw_os_error(),
            Some(libc::EXDEV),
            "the kernel has to stop the walk, not us afterwards"
        );

        std::fs::remove_file(secret).ok();
    }

    #[test]
    fn a_link_to_a_directory_outside_is_refused_the_same_way() {
        let tree = Tree::new();
        let elsewhere = tree.path().parent().expect("a parent").join("outside-dir");
        std::fs::create_dir_all(&elsewhere).expect("a directory");
        std::fs::write(elsewhere.join("loot"), b"x").expect("a file");
        symlink(&elsewhere, tree.path().join("shortcut")).expect("the link");

        let root = tree.root();
        assert_eq!(
            root.dir(&at("/shortcut")).expect_err("no listing").raw_os_error(),
            Some(libc::EXDEV)
        );
        assert_eq!(
            root.open_read(&at("/shortcut/loot")).expect_err("no reading").raw_os_error(),
            Some(libc::EXDEV),
            "the escape is at the first segment and the file behind it stays unreachable"
        );

        std::fs::remove_dir_all(elsewhere).ok();
    }

    #[test]
    fn a_path_that_only_breaks_out_at_its_second_segment_is_refused_too() {
        let tree = Tree::new();
        symlink("/etc", tree.path().join("plugins/etc")).expect("the link");

        let root = tree.root();
        let refused = root.open_read(&at("/plugins/etc/passwd")).expect_err("no reading");
        assert_eq!(refused.raw_os_error(), Some(libc::EXDEV));
        assert!(Path::new("/etc/passwd").exists(), "the test is only worth something if it exists");
    }

    #[test]
    fn a_link_that_stays_inside_still_works() {
        let tree = Tree::new();
        symlink("config", tree.path().join("plugins/latest")).expect("the link");

        let root = tree.root();
        let mut through = root.open_read(&at("/plugins/latest/one.yml")).expect("this may open");
        let mut text = String::new();
        io::Read::read_to_string(&mut through, &mut text).expect("the bytes");
        assert_eq!(text, "a: 1\n", "RESOLVE_BENEATH allows what stays beneath");

        let plugins = root.dir(&at("/plugins")).expect("the directory");
        assert_eq!(plugins.meta(b"latest").expect("the entry").kind, Kind::Symlink);
    }

    #[test]
    fn deleting_a_link_leaves_what_it_points_at_alone() {
        let tree = Tree::new();
        symlink("config/one.yml", tree.path().join("plugins/alias.yml")).expect("the link");

        let root = tree.root();
        let plugins = root.dir(&at("/plugins")).expect("the directory");
        plugins.unlink(b"alias.yml").expect("the link goes");

        assert!(!tree.path().join("plugins/alias.yml").is_symlink());
        assert!(tree.path().join("plugins/config/one.yml").exists(), "the target stays");
    }

    #[test]
    fn writing_never_follows_a_link_on_the_last_segment() {
        let tree = Tree::new();
        let outside = tree.path().parent().expect("a parent").join("outside.txt");
        std::fs::write(&outside, b"before").expect("a file");
        symlink(&outside, tree.path().join("bait.txt")).expect("the link");

        let root = tree.root();
        let dir = root.parent_of(&at("/bait.txt")).expect("the root directory");
        let refused = dir.create_file(b"bait.txt").expect_err("no writing through a link");
        assert_eq!(refused.raw_os_error(), Some(libc::EEXIST));
        assert_eq!(std::fs::read(&outside).expect("still there"), b"before");

        std::fs::remove_file(outside).ok();
    }

    #[test]
    fn the_root_itself_resolves_and_lists() {
        let tree = Tree::new();
        let root = tree.root();
        let mut names = root.dir(&RelPath::root()).expect("the root").entries().expect("names");
        names.sort();
        assert_eq!(names, [b"plugins".to_vec(), b"server.properties".to_vec()]);
    }

    #[test]
    fn a_subtree_goes_away_whole_and_a_link_inside_it_stays_a_link() {
        let tree = Tree::new();
        let outside = tree.path().parent().expect("a parent").join("keep-me.txt");
        std::fs::write(&outside, b"keep").expect("a file");
        symlink(&outside, tree.path().join("plugins/config/away.link")).expect("the link");

        let root = tree.root();
        root.dir(&RelPath::root()).expect("the root").remove_tree(b"plugins").expect("it goes");

        assert!(!tree.path().join("plugins").exists());
        assert_eq!(std::fs::read(&outside).expect("still there"), b"keep");
        std::fs::remove_file(outside).ok();
    }

    #[test]
    fn a_part_file_is_renamed_into_place_and_never_seen_half_written() {
        let tree = Tree::new();
        let root = tree.root();
        let dir = root.dir(&RelPath::root()).expect("the root");

        let (part, mut file) = Part::create(dir, "server.properties").expect("a part");
        assert!(String::from_utf8_lossy(part.name()).starts_with(".server.properties.part."));
        io::Write::write_all(&mut file, b"level-name=nether\n").expect("the bytes");
        file.sync_all().expect("the sync");
        drop(file);
        part.commit(b"server.properties", true).expect("the rename");

        assert_eq!(
            std::fs::read_to_string(tree.path().join("server.properties")).expect("the file"),
            "level-name=nether\n"
        );
        let left = std::fs::read_dir(tree.path())
            .expect("the directory")
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().contains(".part."))
            .count();
        assert_eq!(left, 0, "the part file is gone once it has a real name");
    }

    #[test]
    fn a_part_file_that_is_never_committed_takes_itself_away() {
        let tree = Tree::new();
        let root = tree.root();
        let (part, file) = Part::create(root.dir(&RelPath::root()).unwrap(), "big.zip").unwrap();
        let name = part.name().to_vec();
        assert!(tree.path().join(String::from_utf8_lossy(&name).as_ref()).exists());

        drop(file);
        drop(part);
        assert!(
            !tree.path().join(String::from_utf8_lossy(&name).as_ref()).exists(),
            "a broken upload leaves nothing behind"
        );
    }

    #[test]
    fn refusing_to_overwrite_is_one_step_and_not_two() {
        let tree = Tree::new();
        let root = tree.root();
        let dir = root.dir(&RelPath::root()).expect("the root");
        std::fs::write(tree.path().join("a.txt"), b"a").expect("a file");
        std::fs::write(tree.path().join("b.txt"), b"b").expect("a file");

        let refused = dir.rename_to(b"a.txt", &dir, b"b.txt", false).expect_err("no overwrite");
        assert_eq!(refused.raw_os_error(), Some(libc::EEXIST));
        assert_eq!(std::fs::read(tree.path().join("b.txt")).unwrap(), b"b");

        dir.rename_to(b"a.txt", &dir, b"b.txt", true).expect("overwriting is allowed");
        assert_eq!(std::fs::read(tree.path().join("b.txt")).unwrap(), b"a");
    }

    fn open_descriptors() -> usize {
        std::fs::read_dir("/proc/self/fd").map(Iterator::count).unwrap_or(0)
    }

    #[test]
    fn a_committed_part_lets_go_of_the_directory_it_was_written_in() {
        let tree = Tree::new();
        let root = tree.root();
        let (part, file) = Part::create(root.dir(&RelPath::root()).unwrap(), "warm.txt").unwrap();
        drop(file);
        part.commit(b"warm.txt", true).expect("the rename");

        let writes = 256;
        let before = open_descriptors();
        for index in 0..writes {
            let name = format!("file-{index:03}.txt");
            let (part, mut file) =
                Part::create(root.dir(&RelPath::root()).unwrap(), &name).expect("a part");
            io::Write::write_all(&mut file, b"x").expect("the bytes");
            drop(file);
            part.commit(name.as_bytes(), true).expect("the rename");
        }
        let after = open_descriptors();

        assert!(
            after < before + writes / 4,
            "every finished write must give its directory descriptor back, \
             or the panel runs out of them: {before} open before, {after} after {writes} writes"
        );
    }

    #[test]
    fn a_name_that_is_not_utf8_is_findable_exactly_once() {
        let tree = Tree::new();
        let broken = std::ffi::OsString::from_vec(vec![b'b', 0xff, b'd']);
        std::fs::write(tree.path().join(&broken), b"x").expect("a file with a broken name");

        let root = tree.root();
        let dir = root.dir(&RelPath::root()).expect("the root");
        let lossy = String::from_utf8_lossy(&[b'b', 0xff, b'd']).into_owned();

        let found = dir.only_lossy_match(&lossy).expect("a look").expect("exactly one");
        assert_eq!(found, vec![b'b', 0xff, b'd']);
        assert_eq!(dir.only_lossy_match("server.properties").expect("a look"), None);
    }

}
