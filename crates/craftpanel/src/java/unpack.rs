use std::cell::Cell;
use std::ffi::{CStr, CString, OsStr};
use std::io;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};

use super::error::{JavaError, Result};
use super::progress::Progress;

const MAX_ENTRIES: usize = 4_096;
const MAX_BYTES: u64 = 512 * 1024 * 1024;
const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_NAME_BYTES: u64 = 4 * 1024;
const MAX_DEPTH: usize = 16;
const DIRECTORY_BYTES: u64 = 4 * 1024;

const DIRECTORY: i32 = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
const NEW_FILE: i32 = libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW
    | libc::O_CLOEXEC;
const TREE_MODE: u32 = 0o755;
const PROGRAM_MODE: u32 = 0o755;
const PLAIN_MODE: u32 = 0o644;

#[derive(Debug, PartialEq, Eq)]
pub struct Unpacked {
    pub root: PathBuf,
    pub entries: usize,
    pub bytes: u64,
}

pub fn tree(archive: &Path, into: &Path, major: u32, progress: &Progress) -> Result<Unpacked> {
    tree_capped(archive, into, major, progress, MAX_BYTES)
}

fn tree_capped(
    archive: &Path,
    into: &Path,
    major: u32,
    progress: &Progress,
    ceiling: u64,
) -> Result<Unpacked> {
    std::fs::create_dir_all(into).map_err(|err| JavaError::write(into, err))?;
    let beneath = Beneath::open(into).map_err(|err| JavaError::write(into, err))?;

    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(archive)
        .map_err(|err| JavaError::write(archive, err))?;
    let decoder = flate2::read::GzDecoder::new(Counted { inner: file, progress });
    let burst = Cell::new(false);
    let mut tar = tar::Archive::new(Capped { inner: decoder, left: ceiling, burst: &burst });

    let laid = lay_out(&mut tar, &beneath, into, major, ceiling);
    if burst.get() {
        return Err(too_much(
            major,
            format!("more than {ceiling} bytes in the stream itself, whatever its headers say"),
        ));
    }
    laid
}

fn lay_out<R: io::Read>(
    tar: &mut tar::Archive<R>,
    beneath: &Beneath,
    into: &Path,
    major: u32,
    ceiling: u64,
) -> Result<Unpacked> {
    let mut top: Option<PathBuf> = None;
    let mut entries = 0usize;
    let mut bytes = 0u64;
    let mut long_name: Option<Vec<u8>> = None;
    let mut long_target: Option<Vec<u8>> = None;

    for entry in tar.entries().map_err(|err| unreadable(major, err))?.raw(true) {
        let mut entry = entry.map_err(|err| unreadable(major, err))?;

        entries += 1;
        weighed(entries, bytes, major, ceiling)?;

        let kind = entry.header().entry_type();
        let size = entry.size();
        if spoken_for(kind) {
            if size > MAX_NAME_BYTES {
                return Err(too_much(
                    major,
                    format!(
                        "a header announcing {size} bytes of name, where {MAX_NAME_BYTES} is \
                         already more than any path can hold"
                    ),
                ));
            }
            let held = held_aside(&mut entry, major)?;
            if kind.is_gnu_longname() {
                long_name = Some(held);
            } else if kind.is_gnu_longlink() {
                long_target = Some(held);
            }
            continue;
        }

        bytes = bytes.saturating_add(size);
        weighed(entries, bytes, major, ceiling)?;
        if size > MAX_FILE_BYTES {
            return Err(too_much(major, format!("more than {MAX_FILE_BYTES} bytes in one file")));
        }

        let retargeted = long_target.take();
        let named = match long_name.take() {
            Some(held) => confine(Path::new(OsStr::from_bytes(shorn(&held))), major)?,
            None => confine(&entry.path().map_err(|err| unreadable(major, err))?, major)?,
        };
        if named.as_os_str().is_empty() {
            continue;
        }

        let (first, inside) = split(&named);
        match &top {
            None => top = Some(first),
            Some(known) if *known != first => {
                return Err(too_much(
                    major,
                    format!(
                        "two root directories, {} and {}",
                        known.display(),
                        first.display()
                    ),
                ))
            }
            Some(_) => {}
        }
        if inside.as_os_str().is_empty() {
            continue;
        }

        if kind.is_hard_link() {
            return Err(JavaError::Escapes(format!(
                "{}, a hard link, and a Java runtime holds none",
                inside.display()
            )));
        }

        let offered = entry.header().mode().map_err(|err| unreadable(major, err))?;
        let laid = if kind.is_symlink() {
            let target = match &retargeted {
                Some(held) => {
                    std::borrow::Cow::Owned(PathBuf::from(OsStr::from_bytes(shorn(held))))
                }
                None => entry
                    .link_name()
                    .map_err(|err| unreadable(major, err))?
                    .ok_or_else(|| too_much(major, "a link entry without a target".to_owned()))?,
            };
            if !lands_inside(&inside, &target) {
                return Err(JavaError::Escapes(format!(
                    "{}, a link out of the runtime directory to {}",
                    inside.display(),
                    target.display()
                )));
            }
            beneath.link(&inside, &target)
        } else if kind.is_dir() {
            beneath.directory(&inside)
        } else if kind.is_file() {
            beneath.file(&inside, granted(offered), &mut entry)
        } else {
            return Err(too_much(
                major,
                format!("{}, which is no file, directory or link", inside.display()),
            ));
        };
        let dug = laid.map_err(|err| blocked(major, &inside, &into.join(&inside), err))?;
        entries += dug;
        bytes = bytes.saturating_add(dug as u64 * DIRECTORY_BYTES);
        weighed(entries, bytes, major, ceiling)?;
    }

    let root = top.ok_or_else(|| too_much(major, "no entries at all".to_owned()))?;
    Ok(Unpacked { root, entries, bytes })
}

fn weighed(entries: usize, bytes: u64, major: u32, ceiling: u64) -> Result<()> {
    if entries > MAX_ENTRIES {
        return Err(too_much(major, format!("more than {MAX_ENTRIES} entries and directories")));
    }
    if bytes > ceiling {
        return Err(too_much(major, format!("more than {ceiling} unpacked bytes")));
    }
    Ok(())
}

struct Beneath {
    root: OwnedFd,
}

impl Beneath {
    fn open(path: &Path) -> io::Result<Self> {
        let raw = cstring(path.as_os_str().as_bytes())?;
        let root = take_fd(unsafe { libc::open(raw.as_ptr(), DIRECTORY) })?;
        check(unsafe { libc::fchmod(root.as_raw_fd(), TREE_MODE as libc::mode_t) })?;
        Ok(Self { root })
    }

    fn directory(&self, at: &Path) -> io::Result<usize> {
        let (parent, name, dug) = self.leaf(at)?;
        let (_, fresh) = make_dir(parent.as_fd(), &name)?;
        Ok(dug + usize::from(fresh))
    }

    fn file(&self, at: &Path, mode: u32, body: &mut impl io::Read) -> io::Result<usize> {
        let (parent, name, dug) = self.leaf(at)?;
        unbind(parent.as_fd(), &name)?;
        let held = openat(parent.as_fd(), &name, NEW_FILE, mode)?;
        check(unsafe { libc::fchmod(held.as_raw_fd(), mode as libc::mode_t) })?;
        io::copy(body, &mut std::fs::File::from(held))?;
        Ok(dug)
    }

    fn link(&self, at: &Path, target: &Path) -> io::Result<usize> {
        let (parent, name, dug) = self.leaf(at)?;
        unbind(parent.as_fd(), &name)?;
        let target = cstring(target.as_os_str().as_bytes())?;
        check(unsafe { libc::symlinkat(target.as_ptr(), parent.as_raw_fd(), name.as_ptr()) })?;
        Ok(dug)
    }

    fn leaf(&self, at: &Path) -> io::Result<(OwnedFd, CString, usize)> {
        let mut steps = Vec::new();
        for step in at.as_os_str().as_bytes().split(|byte| *byte == b'/') {
            if step.is_empty() || step == b"." || step == b".." {
                return Err(io::Error::from_raw_os_error(libc::EINVAL));
            }
            steps.push(cstring(step)?);
        }

        let (name, leading) = steps.split_last().expect("a split leaves at least one step");
        let mut here = self.root.try_clone()?;
        let mut dug = 0usize;
        for step in leading {
            let (next, fresh) = make_dir(here.as_fd(), step)?;
            here = next;
            dug += usize::from(fresh);
        }
        Ok((here, name.clone(), dug))
    }
}

fn spoken_for(kind: tar::EntryType) -> bool {
    kind.is_gnu_longname()
        || kind.is_gnu_longlink()
        || kind.is_pax_local_extensions()
        || kind.is_pax_global_extensions()
}

fn held_aside(entry: &mut impl io::Read, major: u32) -> Result<Vec<u8>> {
    let mut held = Vec::new();
    let mut only = io::Read::take(entry, MAX_NAME_BYTES);
    io::Read::read_to_end(&mut only, &mut held).map_err(|err| unreadable(major, err))?;
    Ok(held)
}

fn shorn(name: &[u8]) -> &[u8] {
    &name[..name.iter().position(|byte| *byte == 0).unwrap_or(name.len())]
}

fn granted(offered: u32) -> u32 {
    if offered & 0o111 == 0 {
        PLAIN_MODE
    } else {
        PROGRAM_MODE
    }
}

fn make_dir(dir: BorrowedFd<'_>, name: &CStr) -> io::Result<(OwnedFd, bool)> {
    let made = unsafe { libc::mkdirat(dir.as_raw_fd(), name.as_ptr(), TREE_MODE as libc::mode_t) };
    let fresh = match check(made) {
        Err(err) if err.raw_os_error() == Some(libc::EEXIST) => false,
        other => {
            other?;
            true
        }
    };
    let held = openat(dir, name, DIRECTORY, 0).map_err(|err| told_apart(dir, name, err))?;
    check(unsafe { libc::fchmod(held.as_raw_fd(), TREE_MODE as libc::mode_t) })?;
    Ok((held, fresh))
}

fn told_apart(dir: BorrowedFd<'_>, name: &CStr, err: io::Error) -> io::Error {
    let refused = matches!(err.raw_os_error(), Some(libc::ENOTDIR) | Some(libc::ELOOP));
    if refused && is_link(dir, name) {
        return io::Error::from_raw_os_error(libc::ELOOP);
    }
    err
}

fn is_link(dir: BorrowedFd<'_>, name: &CStr) -> bool {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    let seen = unsafe {
        libc::fstatat(dir.as_raw_fd(), name.as_ptr(), stat.as_mut_ptr(), libc::AT_SYMLINK_NOFOLLOW)
    };
    seen == 0 && unsafe { stat.assume_init() }.st_mode & libc::S_IFMT == libc::S_IFLNK
}

fn unbind(dir: BorrowedFd<'_>, name: &CStr) -> io::Result<()> {
    match check(unsafe { libc::unlinkat(dir.as_raw_fd(), name.as_ptr(), 0) }) {
        Err(err) if err.raw_os_error() == Some(libc::ENOENT) => Ok(()),
        other => other,
    }
}

fn openat(dir: BorrowedFd<'_>, name: &CStr, flags: i32, mode: u32) -> io::Result<OwnedFd> {
    take_fd(unsafe { libc::openat(dir.as_raw_fd(), name.as_ptr(), flags, mode as libc::c_uint) })
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

fn take_fd(raw: libc::c_int) -> io::Result<OwnedFd> {
    if raw < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { OwnedFd::from_raw_fd(raw) })
    }
}

struct Counted<'a, R> {
    inner: R,
    progress: &'a Progress,
}

impl<R: io::Read> io::Read for Counted<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buf)?;
        self.progress.advanced(read as u64);
        Ok(read)
    }
}

struct Capped<'a, R> {
    inner: R,
    left: u64,
    burst: &'a Cell<bool>,
}

impl<R: io::Read> io::Read for Capped<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buf)?;
        match self.left.checked_sub(read as u64) {
            Some(left) => {
                self.left = left;
                Ok(read)
            }
            None => {
                self.burst.set(true);
                Err(io::Error::other("the archive is longer than it may be"))
            }
        }
    }
}

fn confine(path: &Path, major: u32) -> Result<PathBuf> {
    if path.as_os_str().is_empty() {
        return Err(too_much(major, "an entry without a name".to_owned()));
    }

    let mut safe = PathBuf::new();
    let mut deep = 0usize;
    for part in path.components() {
        match part {
            Component::Normal(name) => {
                deep += 1;
                if deep > MAX_DEPTH {
                    return Err(too_much(
                        major,
                        format!(
                            "{}, which is nested deeper than {MAX_DEPTH} directories",
                            path.display()
                        ),
                    ));
                }
                safe.push(name);
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(JavaError::Escapes(format!(
                    "{}, which leaves the directory it is unpacked into",
                    path.display()
                )))
            }
        }
    }
    Ok(safe)
}

fn split(name: &Path) -> (PathBuf, PathBuf) {
    let mut parts = name.components();
    let first = parts.next().map(|part| PathBuf::from(part.as_os_str())).unwrap_or_default();
    (first, parts.as_path().to_path_buf())
}

fn lands_inside(link: &Path, target: &Path) -> bool {
    if target.as_os_str().is_empty() {
        return false;
    }

    let mut depth = link.parent().map(|above| above.components().count()).unwrap_or_default();
    let mut through_a_name = false;
    for part in target.components() {
        match part {
            Component::CurDir => {}
            Component::Normal(_) => {
                depth += 1;
                through_a_name = true;
            }
            Component::ParentDir if through_a_name || depth == 0 => return false,
            Component::ParentDir => depth -= 1,
            Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    true
}

fn unreadable(major: u32, err: io::Error) -> JavaError {
    JavaError::Malformed { major, reason: format!("it cannot be read: {err}") }
}

fn too_much(major: u32, reason: String) -> JavaError {
    JavaError::Malformed { major, reason }
}

fn blocked(major: u32, inside: &Path, at: &Path, err: io::Error) -> JavaError {
    match err.raw_os_error() {
        Some(libc::ELOOP) => JavaError::Escapes(format!(
            "{}, which would be written through a link the archive laid down itself",
            inside.display()
        )),
        Some(libc::ENOTDIR) | Some(libc::EEXIST) | Some(libc::EISDIR) => too_much(
            major,
            format!("{}, under a name it already gave to something else", inside.display()),
        ),
        _ => JavaError::write(at, err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    use crate::java::harness::{self, a_data_dir, FakeAdoptium, Scratch};
    use crate::java::Runtimes;

    const VERSION: &str = "21.0.12+7";
    const PLANTED: &str = "PLANTED.txt";
    const A_CHILD: &str = "CRAFTPANEL_JAVA_UNPACK_UNDER_A_UMASK";

    fn root() -> String {
        format!("jdk-{VERSION}-jre")
    }

    fn release() -> Vec<u8> {
        format!("IMPLEMENTOR=\"Eclipse Adoptium\"\nJAVA_VERSION=\"{VERSION}\"\n").into_bytes()
    }

    fn planted_below(dir: &Path) -> Vec<PathBuf> {
        walkdir::WalkDir::new(dir)
            .follow_links(false)
            .into_iter()
            .filter_map(|found| found.ok())
            .map(|found| found.into_path())
            .filter(|path| path.file_name() == Some(std::ffi::OsStr::new(PLANTED)))
            .collect()
    }

    async fn nothing_gets_out(archive: Vec<u8>) -> JavaError {
        let dir = a_data_dir();
        let upstream = FakeAdoptium::started().await;
        upstream.offer(21, VERSION, archive);

        let runtimes = Runtimes::with_base(dir.path(), upstream.base()).expect("a client");
        let outcome = runtimes.install(21).await;

        let held = dir.path().join("runtimes");
        assert!(planted_below(dir.path()).is_empty(), "planted {:?}", planted_below(dir.path()));
        assert!(!held.join(PLANTED).exists(), "runtimes/ holds a file the archive smuggled in");
        assert!(!held.join(".java-21.new").exists(), "the staging is gone");
        assert!(!held.join("java-21").exists(), "and no runtime was laid down");
        outcome.expect_err("the archive must be refused")
    }

    const PAX_RECORDS: &[u8] = b"52 comment=6a3f1b2c4d5e6f708192a3b4c5d6e7f80912a3b4\n";

    fn a_declared_size(name: &str, kind: tar::EntryType) -> Vec<u8> {
        harness::tarball(|builder| {
            harness::declaring(builder, name, kind, 3 * 1024 * 1024 * 1024);
            harness::file(builder, &format!("{}/bin/java", root()), b"#!/bin/sh\n", 0o755);
        })
    }

    #[tokio::test]
    async fn a_size_is_weighed_before_the_entry_declaring_it_is_stepped_over() {
        let shapes = [
            (root(), tar::EntryType::Regular, "unpacked bytes"),
            ("./".to_owned(), tar::EntryType::Directory, "unpacked bytes"),
            ("pax_global_header".to_owned(), tar::EntryType::XGlobalHeader, "bytes of name"),
        ];

        for (name, kind, weighed) in shapes {
            let refusal = nothing_gets_out(a_declared_size(&name, kind)).await;
            assert_eq!(refusal.code(), "java_archive_rejected", "{name}");
            assert!(
                refusal.to_string().contains(weighed),
                "{name} was let past the ceiling: {refusal}"
            );
        }
    }

    #[test]
    fn a_name_the_tar_crate_would_swallow_whole_is_weighed_before_it_gets_to() {
        let dir = a_data_dir();
        let swollen = harness::tarball(|builder| {
            harness::file(builder, &format!("{}/bin/java", root()), b"#!/bin/sh\n", 0o755);
            harness::file(builder, &format!("{}/release", root()), &release(), 0o644);
            harness::long_name_of(builder, 64 * 1024);
        });

        let refusal = beneath_a_ceiling(dir.path(), "swollen", swollen, MAX_BYTES)
            .expect_err("a name of 64 KiB is no name");
        assert_eq!(refusal.code(), "java_archive_rejected");
        assert!(refusal.to_string().contains("65536 bytes of name"), "{refusal}");
    }

    #[test]
    fn the_stream_is_weighed_too_where_every_header_of_it_is_honest() {
        let dir = a_data_dir();
        let flat = harness::tarball(|builder| {
            harness::file(builder, &format!("{}/java", root()), b"#!/bin/sh\n", 0o755);
            harness::file(builder, &format!("{}/release", root()), &release(), 0o644);
        });

        let laid = beneath_a_ceiling(dir.path(), "flat", flat.clone(), 16 * 1024).expect("a tree");
        assert_eq!(laid.entries, 2, "two entries and no directory between them");
        assert!(laid.bytes < 1024, "{} bytes are named in the headers", laid.bytes);

        let refusal = beneath_a_ceiling(dir.path(), "padded", flat, 1024)
            .expect_err("headers and padding are stream as much as bodies are");
        assert_eq!(refusal.code(), "java_archive_rejected");
        assert!(refusal.to_string().contains("in the stream itself"), "{refusal}");
    }

    #[test]
    fn a_directory_the_archive_never_names_is_counted_and_charged_for_all_the_same() {
        let dir = a_data_dir();
        let nested = harness::tarball(|builder| {
            harness::file(builder, &format!("{}/bin/java", root()), b"#!/bin/sh\n", 0o755);
            harness::file(builder, &format!("{}/release", root()), &release(), 0o644);
        });

        let laid = beneath_a_ceiling(dir.path(), "nested", nested, MAX_BYTES).expect("a tree");

        assert_eq!(laid.entries, 3, "two entries, and the bin/ the archive never names");
        assert_eq!(
            laid.bytes,
            DIRECTORY_BYTES + 10 + release().len() as u64,
            "the two bodies and the block the directory takes"
        );
    }

    fn beneath_a_ceiling(
        dir: &Path,
        name: &str,
        archive: Vec<u8>,
        ceiling: u64,
    ) -> Result<Unpacked> {
        let at = dir.join(format!("{name}.tar.gz"));
        std::fs::write(&at, archive).expect("an archive on disk");
        tree_capped(&at, &dir.join(name), 21, &Progress::default(), ceiling)
    }

    #[test]
    fn the_stream_is_cut_at_the_read_that_bursts_the_ceiling_and_says_it_was() {
        use std::io::Read;

        let burst = Cell::new(false);
        let mut capped = Capped { inner: &b"0123456789"[..], left: 6, burst: &burst };
        let mut four = [0u8; 4];

        assert_eq!(capped.read(&mut four).expect("the first four"), 4);
        assert!(!burst.get());
        capped.read(&mut four).expect_err("the next four are two over the six");
        assert!(burst.get(), "and the unpacking is told which ceiling it was");
    }

    #[tokio::test]
    async fn a_pax_global_header_in_front_is_stepped_over_and_the_runtime_lands() {
        let dir = a_data_dir();
        let upstream = FakeAdoptium::started().await;
        let archive = harness::tarball(|builder| {
            harness::pax_global_header(builder, PAX_RECORDS);
            harness::directory(builder, &format!("{}/", root()), 0o755);
            harness::file(builder, &format!("{}/bin/java", root()), b"#!/bin/sh\n", 0o755);
            harness::file(builder, &format!("{}/release", root()), &release(), 0o644);
        });
        upstream.offer(21, VERSION, archive);

        let runtimes = Runtimes::with_base(dir.path(), upstream.base()).expect("a client");
        let home = runtimes.install(21).await.expect("a runtime").home;

        assert_eq!(mode_of(&home.join("bin").join("java")), 0o755);
        assert!(!home.join("pax_global_header").exists(), "and it is no file of the runtime");
    }

    #[tokio::test]
    async fn the_leading_dot_that_plain_tar_writes_is_stepped_over_too() {
        let dir = a_data_dir();
        let upstream = FakeAdoptium::started().await;
        let archive = harness::tarball(|builder| {
            harness::raw_directory(builder, b"./");
            harness::raw_directory(builder, format!("./{}/", root()).as_bytes());
            harness::raw_file(
                builder,
                format!("./{}/bin/java", root()).as_bytes(),
                b"#!/bin/sh\n",
                0o755,
            );
            harness::raw_file(
                builder,
                format!("./{}/release", root()).as_bytes(),
                &release(),
                0o644,
            );
        });
        upstream.offer(21, VERSION, archive);

        let runtimes = Runtimes::with_base(dir.path(), upstream.base()).expect("a client");
        let home = runtimes.install(21).await.expect("a runtime").home;

        assert_eq!(mode_of(&home.join("bin").join("java")), 0o755);
        assert_eq!(std::fs::read(home.join("release")).expect("the release file"), release());
    }

    #[tokio::test]
    async fn stepping_over_the_two_of_them_steps_over_no_second_root() {
        let behind_a_header = harness::tarball(|builder| {
            harness::pax_global_header(builder, PAX_RECORDS);
            harness::file(builder, &format!("{}/bin/java", root()), b"#!/bin/sh\n", 0o755);
            harness::file(builder, "somewhere-else/bin/java", b"#!/bin/sh\n", 0o755);
        });
        let behind_a_dot = harness::tarball(|builder| {
            harness::raw_directory(builder, b"./");
            harness::raw_file(
                builder,
                format!("./{}/bin/java", root()).as_bytes(),
                b"#!/bin/sh\n",
                0o755,
            );
            harness::raw_file(builder, b"./somewhere-else/bin/java", b"#!/bin/sh\n", 0o755);
        });

        for archive in [behind_a_header, behind_a_dot] {
            let refusal = nothing_gets_out(archive).await;
            assert_eq!(refusal.code(), "java_archive_rejected");
            assert!(refusal.to_string().contains("two root directories"), "{refusal}");
        }
    }

    #[test]
    fn the_only_thing_the_archive_is_believed_about_is_whether_it_is_a_program() {
        for offered in [0o755, 0o777, 0o4755, 0o2711, 0o111, 0o1755] {
            assert_eq!(granted(offered), 0o755, "{offered:o}");
        }
        for offered in [0o644, 0o666, 0o444, 0o600, 0o000, 0o4644] {
            assert_eq!(granted(offered), 0o644, "{offered:o}");
        }
    }

    #[test]
    fn a_name_is_kept_only_if_every_step_of_it_leads_further_in() {
        let kept = confine(Path::new("jdk-21/bin/java"), 21).unwrap();
        assert_eq!(kept, Path::new("jdk-21/bin/java"));
        let tidied = confine(Path::new("./jdk-21/./release"), 21).unwrap();
        assert_eq!(tidied, Path::new("jdk-21/release"));

        for name in ["../escaped", "jdk-21/../../escaped", "/etc/shadow"] {
            let refusal = confine(Path::new(name), 21).expect_err(name);
            assert_eq!(refusal.code(), "java_archive_rejected", "{name}");
        }

        for name in [".", "./"] {
            let itself = confine(Path::new(name), 21).expect(name);
            assert!(itself.as_os_str().is_empty(), "{name} names the archive and nothing in it");
        }
        let refusal = confine(Path::new(""), 21).expect_err("an entry without a name");
        assert_eq!(refusal.code(), "java_archive_rejected");
    }

    #[test]
    fn the_root_directory_comes_off_the_front_and_leaves_nothing_of_itself() {
        let (first, inside) = split(Path::new("jdk-21/bin/java"));
        assert_eq!(first, Path::new("jdk-21"));
        assert_eq!(inside, Path::new("bin/java"));

        let (first, inside) = split(Path::new("jdk-21"));
        assert_eq!(first, Path::new("jdk-21"));
        assert!(inside.as_os_str().is_empty(), "the root entry itself unpacks to nothing");
    }

    #[test]
    fn a_link_is_judged_by_the_archive_alone_and_never_by_what_lies_on_the_disk() {
        assert!(lands_inside(Path::new("lib/server/libjsig.so"), Path::new("../libjsig.so")));
        assert!(lands_inside(Path::new("man/ja"), Path::new("ja_JP.UTF-8")));
        assert!(lands_inside(Path::new("legal/java.xml/LICENSE"), Path::new("../java.base/LICENSE")));

        assert!(!lands_inside(Path::new("keys"), Path::new("/etc/shadow")));
        assert!(!lands_inside(Path::new("out"), Path::new("../elsewhere")));
        assert!(!lands_inside(Path::new("lib/out"), Path::new("../../elsewhere")));
        assert!(!lands_inside(Path::new("lib/in"), Path::new("")));
    }

    #[test]
    fn a_step_back_through_a_name_is_refused_because_the_name_may_be_a_door() {
        assert!(!lands_inside(Path::new("lib/out"), Path::new("door/../..")));
        assert!(!lands_inside(Path::new("lib/out"), Path::new("door/../../elsewhere")));
        assert!(!lands_inside(Path::new("lib/deep/out"), Path::new("../door/../../..")));
        assert!(
            lands_inside(Path::new("lib/in"), Path::new("door/lib/libjsig.so")),
            "a name that only leads further in is still a name"
        );
    }

    #[tokio::test]
    async fn the_door_that_the_archive_opens_after_the_way_out_plants_nothing() {
        let archive = harness::tarball(|builder| {
            harness::file(builder, &format!("{}/bin/java", root()), b"#!/bin/sh\n", 0o755);
            harness::link(builder, &format!("{}/lib/out", root()), "door/../..");
            harness::link(builder, &format!("{}/lib/door", root()), "..");
            harness::file(builder, &format!("{}/lib/out/{PLANTED}", root()), b"owned", 0o644);
        });

        let refusal = nothing_gets_out(archive).await;
        assert_eq!(refusal.code(), "java_archive_rejected");
        assert!(refusal.to_string().contains("a link out of the runtime"), "{refusal}");
    }

    #[tokio::test]
    async fn the_same_way_out_with_the_door_laid_down_first_plants_nothing_either() {
        let archive = harness::tarball(|builder| {
            harness::file(builder, &format!("{}/bin/java", root()), b"#!/bin/sh\n", 0o755);
            harness::link(builder, &format!("{}/lib/door", root()), "..");
            harness::link(builder, &format!("{}/lib/out", root()), "door/../..");
            harness::file(builder, &format!("{}/lib/out/{PLANTED}", root()), b"owned", 0o644);
        });

        let refusal = nothing_gets_out(archive).await;
        assert_eq!(refusal.code(), "java_archive_rejected");
        assert!(refusal.to_string().contains("a link out of the runtime"), "{refusal}");
    }

    #[tokio::test]
    async fn doors_stacked_one_behind_the_other_get_no_further_than_one() {
        let archive = harness::tarball(|builder| {
            harness::file(builder, &format!("{}/bin/java", root()), b"#!/bin/sh\n", 0o755);
            harness::link(builder, &format!("{}/lib/a", root()), "..");
            harness::link(builder, &format!("{}/lib/a/c", root()), "..");
            harness::link(builder, &format!("{}/lib/a/c/e", root()), "..");
            harness::file(builder, &format!("{}/lib/a/c/e/{PLANTED}", root()), b"owned", 0o644);
        });

        let refusal = nothing_gets_out(archive).await;
        assert_eq!(refusal.code(), "java_archive_rejected");
        assert!(refusal.to_string().contains("through a link the archive laid down"), "{refusal}");
    }

    #[tokio::test]
    async fn not_even_a_door_that_leads_back_inside_is_walked_through() {
        let archive = harness::tarball(|builder| {
            harness::file(builder, &format!("{}/bin/java", root()), b"#!/bin/sh\n", 0o755);
            harness::link(builder, &format!("{}/lib/door", root()), "..");
            harness::file(builder, &format!("{}/lib/door/{PLANTED}", root()), b"owned", 0o644);
        });

        let refusal = nothing_gets_out(archive).await;
        assert_eq!(refusal.code(), "java_archive_rejected");
        assert!(refusal.to_string().contains("through a link the archive laid down"), "{refusal}");
    }

    #[tokio::test]
    async fn a_file_that_the_archive_hides_behind_one_of_its_own_files_is_refused() {
        let archive = harness::tarball(|builder| {
            harness::file(builder, &format!("{}/bin/java", root()), b"#!/bin/sh\n", 0o755);
            harness::file(builder, &format!("{}/lib/note", root()), b"a file", 0o644);
            harness::file(builder, &format!("{}/lib/note/{PLANTED}", root()), b"owned", 0o644);
        });

        let refusal = nothing_gets_out(archive).await;
        assert_eq!(refusal.code(), "java_archive_rejected");
        assert!(refusal.to_string().contains("already gave to something else"), "{refusal}");
    }

    #[tokio::test]
    async fn a_file_that_the_archive_lays_over_one_of_its_own_directories_is_refused() {
        let archive = harness::tarball(|builder| {
            harness::file(builder, &format!("{}/bin/java", root()), b"#!/bin/sh\n", 0o755);
            harness::directory(builder, &format!("{}/lib/notes", root()), 0o755);
            harness::file(builder, &format!("{}/lib/notes", root()), b"owned", 0o644);
        });

        let dir = a_data_dir();
        let upstream = FakeAdoptium::started().await;
        upstream.offer(21, VERSION, archive);

        let runtimes = Runtimes::with_base(dir.path(), upstream.base()).expect("a client");
        let refusal = runtimes.install(21).await.expect_err("the archive contradicts itself");

        assert_eq!(refusal.code(), "java_archive_rejected");
        assert!(refusal.to_string().contains("already gave to something else"), "{refusal}");
    }

    #[tokio::test]
    async fn the_shape_a_real_temurin_carries_is_laid_down_whole() {
        let dir = a_data_dir();
        let upstream = FakeAdoptium::started().await;
        let archive = harness::tarball(|builder| {
            harness::directory(builder, &format!("{}/", root()), 0o755);
            harness::directory(builder, &format!("{}/bin", root()), 0o755);
            harness::file(builder, &format!("{}/bin/java", root()), b"#!/bin/sh\n", 0o755);
            harness::file(builder, &format!("{}/release", root()), &release(), 0o644);
            harness::link(
                builder,
                &format!("{}/legal/java.xml/LICENSE", root()),
                "../java.base/LICENSE",
            );
            harness::file(
                builder,
                &format!("{}/legal/java.base/LICENSE", root()),
                b"the licence",
                0o444,
            );
            harness::file(builder, &format!("{}/lib/libjsig.so", root()), b"a library", 0o644);
            harness::link(builder, &format!("{}/lib/server/libjsig.so", root()), "../libjsig.so");
        });
        upstream.offer(21, VERSION, archive);

        let runtimes = Runtimes::with_base(dir.path(), upstream.base()).expect("a client");
        let home = runtimes.install(21).await.expect("a runtime").home;

        let borrowed = home.join("legal").join("java.xml").join("LICENSE");
        assert!(borrowed.symlink_metadata().expect("the link").file_type().is_symlink());
        assert_eq!(
            std::fs::read(&borrowed).expect("through the link"),
            b"the licence",
            "a link that names its target before the archive lays it down still reads"
        );
        assert_eq!(
            std::fs::read(home.join("lib").join("server").join("libjsig.so")).expect("the library"),
            b"a library"
        );
        assert_eq!(mode_of(&home), 0o755, "the tree is reachable whatever the umask is");
        assert_eq!(mode_of(&home.join("bin")), 0o755);
        assert_eq!(mode_of(&home.join("bin").join("java")), 0o755);
        assert_eq!(mode_of(&home.join("release")), 0o644);
        harness::nothing_is_loose(&home);
        assert_eq!(
            mode_of(&home.join("legal").join("java.base").join("LICENSE")),
            0o644,
            "a file the archive marked read-only is still only the owner's to write"
        );
    }

    fn mode_of(path: &Path) -> u32 {
        std::fs::symlink_metadata(path).expect("the entry").permissions().mode() & 0o7777
    }

    fn an_archive_of_loose_modes() -> Vec<u8> {
        harness::tarball(|builder| {
            harness::directory(builder, &format!("{}/", root()), 0o777);
            harness::directory(builder, &format!("{}/bin", root()), 0o707);
            harness::file(builder, &format!("{}/bin/java", root()), b"#!/bin/sh\n", 0o4755);
            harness::file(builder, &format!("{}/bin/keytool", root()), b"#!/bin/sh\n", 0o777);
            harness::file(builder, &format!("{}/release", root()), &release(), 0o666);
            harness::file(builder, &format!("{}/lib/libjvm.so", root()), b"a library", 0o666);
            harness::file(builder, &format!("{}/lib/modules", root()), b"the modules", 0o444);
            harness::file(builder, &format!("{}/lib/jspawnhelper", root()), b"a helper", 0o2711);
            harness::link(builder, &format!("{}/lib/server/libjvm.so", root()), "../libjvm.so");
        })
    }

    async fn installed_from(archive: Vec<u8>) -> (Scratch, PathBuf) {
        let dir = a_data_dir();
        let upstream = FakeAdoptium::started().await;
        upstream.offer(21, VERSION, archive);

        let runtimes = Runtimes::with_base(dir.path(), upstream.base()).expect("a client");
        let home = runtimes.install(21).await.expect("a runtime").home;
        (dir, home)
    }

    #[tokio::test]
    async fn no_mode_the_archive_offers_group_or_others_is_taken() {
        let (_dir, home) = installed_from(an_archive_of_loose_modes()).await;

        assert_eq!(mode_of(&home), 0o755, "0o777 on the root of the archive");
        assert_eq!(mode_of(&home.join("bin")), 0o755, "0o707 on a directory");
        assert_eq!(mode_of(&home.join("bin").join("java")), 0o755, "0o4755 on a program");
        assert_eq!(mode_of(&home.join("bin").join("keytool")), 0o755, "0o777 on a program");
        assert_eq!(mode_of(&home.join("release")), 0o644, "0o666 on a plain file");
        assert_eq!(mode_of(&home.join("lib")), 0o755, "a directory the archive never names");
        assert_eq!(mode_of(&home.join("lib").join("libjvm.so")), 0o644, "0o666 on a library");
        assert_eq!(mode_of(&home.join("lib").join("modules")), 0o644, "0o444 on a plain file");
        assert_eq!(
            mode_of(&home.join("lib").join("jspawnhelper")),
            0o755,
            "0o2711 on a program"
        );
    }

    #[tokio::test]
    async fn every_step_of_the_installed_tree_is_walked_and_none_of_it_is_loose() {
        let (_dir, home) = installed_from(an_archive_of_loose_modes()).await;
        harness::nothing_is_loose(&home);
    }

    #[tokio::test]
    async fn a_temurin_shaped_archive_leaves_nothing_loose_either() {
        let (_dir, home) = installed_from(harness::a_jre(VERSION)).await;
        harness::nothing_is_loose(&home);
    }

    #[tokio::test]
    async fn the_modes_are_the_writers_own_and_never_the_umasks() {
        let Some(name) = module_path!().split_once("::").map(|(_, rest)| rest) else {
            panic!("a test module always sits below its crate");
        };
        if std::env::var_os(A_CHILD).is_none() {
            let ran = std::process::Command::new(std::env::current_exe().expect("this binary"))
                .args(["--exact", &format!("{name}::the_modes_are_the_writers_own_and_never_the_umasks")])
                .arg("--nocapture")
                .env(A_CHILD, "0077")
                .output()
                .expect("this binary again, in a process of its own");
            let said = String::from_utf8_lossy(&ran.stdout).into_owned()
                + &String::from_utf8_lossy(&ran.stderr);
            assert!(ran.status.success(), "{said}");
            assert!(said.contains("1 passed"), "the child ran no test at all: {said}");
            return;
        }

        unsafe { libc::umask(0o077) };
        let (_dir, home) = installed_from(an_archive_of_loose_modes()).await;
        harness::nothing_is_loose(&home);
        assert_eq!(
            mode_of(home.parent().expect("the runtimes directory above it")),
            0o755,
            "a game account walks through runtimes/ to reach bin/java"
        );
    }

    #[tokio::test]
    async fn a_name_and_a_target_too_long_for_a_header_are_taken_from_the_ones_that_carry_them() {
        let long_name = "a".repeat(120);
        let long_target = "c".repeat(120);
        let archive = harness::tarball(|builder| {
            harness::file(builder, &format!("{}/bin/java", root()), b"#!/bin/sh\n", 0o755);
            harness::file(builder, &format!("{}/release", root()), &release(), 0o644);
            harness::file(
                builder,
                &format!("{}/legal/{long_name}/LICENSE", root()),
                b"the licence",
                0o644,
            );
            harness::file(
                builder,
                &format!("{}/lib/{long_target}", root()),
                b"a library",
                0o644,
            );
            harness::link(builder, &format!("{}/lib/there", root()), &long_target);
        });

        let (_dir, home) = installed_from(archive).await;

        let licence = home.join("legal").join(&long_name).join("LICENSE");
        assert_eq!(std::fs::read(&licence).expect("the licence"), b"the licence");
        assert_eq!(
            std::fs::read_link(home.join("lib").join("there")).expect("the link"),
            Path::new(&long_target)
        );
        assert_eq!(
            std::fs::read(home.join("lib").join("there")).expect("through the link"),
            b"a library"
        );
    }

    #[tokio::test]
    async fn a_setuid_bit_in_the_archive_does_not_survive_the_unpacking() {
        let dir = a_data_dir();
        let upstream = FakeAdoptium::started().await;
        let archive = harness::tarball(|builder| {
            harness::file(builder, &format!("{}/bin/java", root()), b"#!/bin/sh\n", 0o4755);
            harness::file(builder, &format!("{}/release", root()), &release(), 0o644);
        });
        upstream.offer(21, VERSION, archive);

        let runtimes = Runtimes::with_base(dir.path(), upstream.base()).expect("a client");
        let home = runtimes.install(21).await.expect("a runtime").home;

        assert_eq!(mode_of(&home.join("bin").join("java")), 0o755);
    }
}
