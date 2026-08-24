use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::SystemTime;

use anyhow::{bail, Context, Result};

const LEVEL: i32 = 3;

const SKIPPED_DIRS: &[&str] = &["logs", "crash-reports", "cache", crate::ops::WORK_DIR];

const SKIPPED_SUFFIX: &str = ".log.gz";

#[derive(Debug, Default)]
pub struct Plan {
    pub entries: Vec<Entry>,
    pub bytes: u64,
    pub newest: Option<SystemTime>,
}

#[derive(Debug)]
pub struct Entry {
    pub path: PathBuf,
    pub kind: Kind,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Kind {
    Directory,
    File { bytes: u64 },
    Link { target: PathBuf },
}

#[derive(Debug, Default)]
pub struct Progress {
    bytes: AtomicU64,
    done: AtomicU64,
    files: AtomicU64,
    cancelled: AtomicBool,
    holdup: std::sync::Mutex<Option<String>>,
}

impl Progress {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn waiting(&self, why: String) {
        *self.holdup.lock().expect("the holdup") = Some(why);
    }

    pub fn moving_again(&self) {
        *self.holdup.lock().expect("the holdup") = None;
    }

    pub fn holdup(&self) -> Option<String> {
        self.holdup.lock().expect("the holdup").clone()
    }

    pub fn back_to(&self, bytes: u64) {
        self.bytes.store(bytes, Ordering::Relaxed);
        self.done.store(bytes, Ordering::Relaxed);
    }

    pub fn add_bytes(&self, bytes: u64) {
        self.bytes.fetch_add(bytes, Ordering::Relaxed);
        self.done.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub fn bytes(&self) -> u64 {
        self.bytes.load(Ordering::Relaxed)
    }

    pub fn done(&self) -> u64 {
        self.done.load(Ordering::Relaxed)
    }

    pub fn files(&self) -> u64 {
        self.files.load(Ordering::Relaxed)
    }
}

#[derive(Debug, thiserror::Error)]
#[error("the run was called off")]
pub struct Cancelled;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct Escapes(String);

pub fn survey(root: &Path) -> Result<Plan> {
    let mut plan = Plan::default();
    if !root.is_dir() {
        return Ok(plan);
    }

    let walk = walkdir::WalkDir::new(root).follow_links(false).min_depth(1).sort_by_file_name();
    for step in walk {
        let step = match step {
            Ok(step) => step,
            Err(err) => {
                tracing::warn!("skipping an unreadable entry: {err}");
                continue;
            }
        };
        let Ok(path) = step.path().strip_prefix(root).map(Path::to_path_buf) else {
            continue;
        };
        if is_skipped(&path) {
            continue;
        }

        let kind = step.file_type();
        let entry = if kind.is_dir() {
            Entry { path, kind: Kind::Directory }
        } else if kind.is_symlink() {
            match link_inside(root, step.path()) {
                Some(target) => Entry { path, kind: Kind::Link { target } },
                None => continue,
            }
        } else if kind.is_file() {
            let bytes = step.metadata().map(|meta| meta.len()).unwrap_or_default();
            plan.bytes += bytes;
            Entry { path, kind: Kind::File { bytes } }
        } else {
            continue;
        };

        if let Ok(modified) = step.metadata().map_err(io::Error::from).and_then(|meta| meta.modified())
        {
            plan.newest = Some(plan.newest.map_or(modified, |newest| newest.max(modified)));
        }
        plan.entries.push(entry);
    }

    Ok(plan)
}

pub fn pack(root: &Path, plan: &Plan, archive: &Path, progress: &Progress) -> Result<u64> {
    if let Some(parent) = archive.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("making {}", parent.display()))?;
    }
    let file = std::fs::File::create(archive)
        .with_context(|| format!("creating {}", archive.display()))?;

    let encoder = zstd::Encoder::new(file, LEVEL)?;
    let mut builder = tar::Builder::new(encoder);
    builder.follow_symlinks(false);

    for entry in &plan.entries {
        if progress.is_cancelled() {
            return Err(Cancelled.into());
        }
        let full = root.join(&entry.path);
        let written = match &entry.kind {
            Kind::Directory => {
                builder.append_path_with_name(&full, &entry.path).map(|()| 0)
            }
            Kind::File { bytes } => append_file(&mut builder, &full, &entry.path).map(|()| *bytes),
            Kind::Link { target } => {
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_size(0);
                header.set_mode(0o777);
                builder.append_link(&mut header, &entry.path, target).map(|()| 0)
            }
        };
        match written {
            Ok(bytes) => {
                progress.bytes.fetch_add(bytes, Ordering::Relaxed);
                progress.done.fetch_add(bytes, Ordering::Relaxed);
                progress.files.fetch_add(1, Ordering::Relaxed);
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                tracing::warn!("{} went away while packing: {err}", full.display());
            }
            Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
                tracing::warn!("{} could not be read and is not in the backup: {err}", full.display());
            }
            Err(err) => {
                return Err(err).with_context(|| format!("packing {}", full.display()))
            }
        }
    }

    let file = builder.into_inner()?.finish()?;
    file.sync_all()?;
    Ok(std::fs::metadata(archive)?.len())
}

fn append_file<W: io::Write>(
    builder: &mut tar::Builder<W>,
    full: &Path,
    name: &Path,
) -> io::Result<()> {
    let file = std::fs::File::open(full)?;
    let mut header = tar::Header::new_gnu();
    header.set_metadata(&file.metadata()?);

    let promised = header.size()?;
    builder.append_data(&mut header, name, Exactly { inner: file, left: promised })
}

struct Exactly<R> {
    inner: R,
    left: u64,
}

impl<R: io::Read> io::Read for Exactly<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.left == 0 {
            return Ok(0);
        }
        let wanted = self.left.min(buf.len() as u64) as usize;
        let read = self.inner.read(&mut buf[..wanted])?;
        let filled = if read == 0 {
            buf[..wanted].fill(0);
            wanted
        } else {
            read
        };
        self.left -= filled as u64;
        Ok(filled)
    }
}

pub fn unpack(archive: &Path, into: &Path, progress: &Progress) -> Result<()> {
    std::fs::create_dir_all(into).with_context(|| format!("making {}", into.display()))?;
    let root = into.canonicalize().with_context(|| format!("resolving {}", into.display()))?;

    let file = std::fs::File::open(archive)
        .with_context(|| format!("opening {}", archive.display()))?;
    let decoder = zstd::Decoder::new(Counted { inner: file, progress })?;
    let mut tar = tar::Archive::new(decoder);
    tar.set_overwrite(true);

    for entry in tar.entries()? {
        if progress.is_cancelled() {
            return Err(Cancelled.into());
        }
        let mut entry = entry?;
        let inside = confine(&entry.path()?)?;
        let target = root.join(&inside);

        let kind = entry.header().entry_type();
        if kind.is_hard_link() {
            return Err(Escapes(format!(
                "{} is a hard link, and a backup holds none",
                inside.display()
            ))
            .into());
        }
        if kind.is_symlink() {
            let link = entry
                .link_name()?
                .ok_or_else(|| anyhow::anyhow!("a link entry without a target"))?;
            confine_link(&root, &inside, &link)?;
        }

        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("making {}", parent.display()))?;
        }

        let bytes = entry.size();
        entry.unpack(&target).with_context(|| format!("unpacking {}", inside.display()))?;
        progress.bytes.fetch_add(bytes, Ordering::Relaxed);
        progress.files.fetch_add(1, Ordering::Relaxed);
    }
    Ok(())
}

pub fn free_bytes(path: &Path) -> Result<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let c_path = CString::new(path.as_os_str().as_bytes())?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    let answered = unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) };
    if answered != 0 {
        return Err(io::Error::last_os_error())
            .with_context(|| format!("asking after the free space on {}", path.display()));
    }
    Ok(stat.f_bavail as u64 * stat.f_frsize as u64)
}

struct Counted<'a, R> {
    inner: R,
    progress: &'a Progress,
}

impl<R: io::Read> io::Read for Counted<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buf)?;
        self.progress.done.fetch_add(read as u64, Ordering::Relaxed);
        Ok(read)
    }
}

fn is_skipped(relative: &Path) -> bool {
    let mut parts = relative.components();
    let Some(Component::Normal(first)) = parts.next() else {
        return true;
    };
    if SKIPPED_DIRS.iter().any(|skipped| first == std::ffi::OsStr::new(skipped)) {
        return true;
    }
    relative.file_name().is_some_and(|name| {
        name.to_string_lossy().to_ascii_lowercase().ends_with(SKIPPED_SUFFIX)
    })
}

fn link_inside(root: &Path, link: &Path) -> Option<PathBuf> {
    let root = root.canonicalize().ok()?;
    let from = link.parent()?.canonicalize().ok()?;
    let target = std::fs::read_link(link).ok()?;
    let resolved = if target.is_absolute() { target } else { from.join(target) };
    let landed = resolved.canonicalize().ok()?;
    landed.starts_with(&root).then(|| step_over(&from, &landed))
}

fn step_over(from: &Path, to: &Path) -> PathBuf {
    let shared = from.components().zip(to.components()).take_while(|(a, b)| a == b).count();
    let mut way = PathBuf::new();
    for _ in shared..from.components().count() {
        way.push("..");
    }
    way.extend(to.components().skip(shared));
    if way.as_os_str().is_empty() {
        way.push(".");
    }
    way
}

fn confine(path: &Path) -> Result<PathBuf> {
    let mut safe = PathBuf::new();
    for part in path.components() {
        match part {
            Component::Normal(name) => safe.push(name),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(Escapes(format!(
                    "{} leaves the directory it is unpacked into",
                    path.display()
                ))
                .into())
            }
        }
    }
    if safe.as_os_str().is_empty() {
        bail!("an entry without a name");
    }
    Ok(safe)
}

fn confine_link(root: &Path, link: &Path, target: &Path) -> Result<()> {
    let refuse = || {
        Err(Escapes(format!(
            "{} points out of the server directory: {}",
            link.display(),
            target.display()
        ))
        .into())
    };
    if target.is_absolute() {
        return refuse();
    }
    let from = root.join(link.parent().unwrap_or(Path::new("")));
    if !land(&from.join(target)).starts_with(root) {
        return refuse();
    }
    Ok(())
}

fn land(path: &Path) -> PathBuf {
    let mut real = PathBuf::new();
    for part in path.components() {
        match part {
            Component::Prefix(prefix) => real.push(prefix.as_os_str()),
            Component::RootDir => real.push("/"),
            Component::CurDir => {}
            Component::ParentDir => {
                real.pop();
            }
            Component::Normal(name) => {
                real.push(name);
                if let Ok(followed) = real.canonicalize() {
                    real = followed;
                }
            }
        }
    }
    real
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::symlink;

    struct Tree(PathBuf);

    impl Tree {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("craftpanel-archive-{name}-{}", crate::model::Id::new()));
            std::fs::create_dir_all(&path).expect("a directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn file(&self, relative: &str, contents: &[u8]) -> PathBuf {
            let path = self.0.join(relative);
            std::fs::create_dir_all(path.parent().expect("a parent")).expect("the parents");
            std::fs::write(&path, contents).expect("a file");
            path
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    fn names(plan: &Plan) -> Vec<String> {
        plan.entries.iter().map(|entry| entry.path.display().to_string()).collect()
    }

    #[test]
    fn the_survey_leaves_out_what_10_names_and_keeps_the_loader_jars() {
        let tree = Tree::new("survey");
        tree.file("server.jar", b"jar");
        tree.file("libraries/net/minecraft/server.jar", b"library");
        tree.file("world/level.dat", b"world");
        tree.file("logs/latest.log", b"log");
        tree.file("logs/2026-08-11-1.log.gz", b"old");
        tree.file("crash-reports/crash.txt", b"crash");
        tree.file("cache/mojang.txt", b"cache");
        tree.file(&format!("{}/0123/half.jar", crate::ops::WORK_DIR), b"partial");
        tree.file("plugins/paper.log.gz", b"rotated");

        let plan = survey(tree.path()).expect("a survey");
        let kept = names(&plan);
        assert!(kept.contains(&"server.jar".to_owned()));
        assert!(kept.contains(&"world/level.dat".to_owned()));
        assert!(
            kept.contains(&"libraries/net/minecraft/server.jar".to_owned()),
            "without the libraries a Forge server does not start again (10)"
        );
        for gone in ["logs", "crash-reports", "cache", crate::ops::WORK_DIR, "plugins/paper.log.gz"]
        {
            assert!(
                !kept.iter().any(|name| name.starts_with(gone)),
                "{gone} should not be in {kept:?}"
            );
        }
        assert_eq!(plan.bytes, 3 + 7 + 5, "the three files that stayed");
    }

    #[test]
    fn a_link_out_of_the_tree_is_dropped_and_one_inside_stays_a_link() {
        let tree = Tree::new("links");
        let outside = Tree::new("elsewhere");
        outside.file("secret.txt", b"not yours");
        tree.file("world/level.dat", b"world");

        symlink(outside.path().join("secret.txt"), tree.path().join("escape.txt"))
            .expect("a link out");
        symlink("world/level.dat", tree.path().join("here.dat")).expect("a link in");

        let plan = survey(tree.path()).expect("a survey");
        let kept = names(&plan);
        assert!(!kept.contains(&"escape.txt".to_owned()), "{kept:?}");
        assert!(kept.contains(&"here.dat".to_owned()));

        let here = plan
            .entries
            .iter()
            .find(|entry| entry.path == Path::new("here.dat"))
            .expect("the link");
        assert_eq!(here.kind, Kind::Link { target: PathBuf::from("world/level.dat") });
    }

    #[test]
    fn what_is_packed_comes_back_out_the_same_and_the_archive_is_a_tar() {
        let tree = Tree::new("round-trip");
        tree.file("server.properties", b"level-name=world\n");
        tree.file("world/level.dat", &[0u8, 1, 2, 3, 4]);
        tree.file("logs/latest.log", b"noise");
        symlink("world/level.dat", tree.path().join("current.dat")).expect("a link");

        let out = Tree::new("out");
        let archive = out.path().join("backup.tar.zst");
        let plan = survey(tree.path()).expect("a survey");
        let progress = Progress::default();
        let size = pack(tree.path(), &plan, &archive, &progress).expect("a packed archive");
        assert!(size > 0);
        assert_eq!(progress.files() as usize, plan.entries.len());

        let back = out.path().join("restored");
        unpack(&archive, &back, &Progress::default()).expect("an unpacked archive");
        assert_eq!(
            std::fs::read(back.join("server.properties")).expect("the file"),
            b"level-name=world\n"
        );
        assert_eq!(std::fs::read(back.join("world/level.dat")).expect("the file"), [0, 1, 2, 3, 4]);
        assert!(!back.join("logs").exists(), "the exclusion list survives the round trip");
        assert_eq!(
            std::fs::read_link(back.join("current.dat")).expect("a link"),
            Path::new("world/level.dat")
        );
    }

    #[test]
    fn an_absolute_link_inside_the_tree_survives_the_round_trip() {
        let tree = Tree::new("absolute-link");
        tree.file("world/level.dat", b"world");
        symlink(tree.path().join("world/level.dat"), tree.path().join("current.dat"))
            .expect("an absolute link in");

        let out = Tree::new("absolute-out");
        let archive = out.path().join("backup.tar.zst");
        let plan = survey(tree.path()).expect("a survey");
        pack(tree.path(), &plan, &archive, &Progress::default()).expect("a packed archive");

        let back = out.path().join("restored");
        unpack(&archive, &back, &Progress::default())
            .expect("what we packed has to come back out again");
        assert_eq!(
            std::fs::read(back.join("current.dat")).expect("through the link"),
            b"world"
        );
    }

    #[test]
    fn an_entry_that_climbs_out_of_the_archive_is_refused() {
        let out = Tree::new("evil");
        let archive = out.path().join("evil.tar.zst");
        write_tar(&archive, |builder| {
            let payload = b"owned";
            let mut header = tar::Header::new_gnu();
            let name = b"../escaped.txt";
            header.as_old_mut().name[..name.len()].copy_from_slice(name);
            header.set_size(payload.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append(&header, &payload[..]).expect("an entry");
        });

        let into = out.path().join("into");
        let refused = unpack(&archive, &into, &Progress::default()).expect_err("it must refuse");
        assert!(refused.to_string().contains("leaves"), "{refused}");
        assert!(!out.path().join("escaped.txt").exists(), "and nothing was written outside");
    }

    #[test]
    fn a_link_in_the_archive_that_points_out_is_refused() {
        let out = Tree::new("evil-link");
        let archive = out.path().join("evil.tar.zst");
        write_tar(&archive, |builder| {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_size(0);
            header.set_mode(0o777);
            builder.append_link(&mut header, "config/keys", "/etc/shadow").expect("a link");
        });

        let into = out.path().join("into");
        let refused = unpack(&archive, &into, &Progress::default()).expect_err("it must refuse");
        assert!(refused.to_string().contains("/etc/shadow"), "{refused}");
        assert!(!into.join("config/keys").exists());
    }

    #[test]
    fn a_chain_of_links_cannot_walk_out_of_the_tree_either() {
        let out = Tree::new("evil-chain");
        let archive = out.path().join("evil.tar.zst");
        write_tar(&archive, |builder| {
            let mut door = tar::Header::new_gnu();
            door.set_entry_type(tar::EntryType::Symlink);
            door.set_size(0);
            door.set_mode(0o777);
            builder.append_link(&mut door, "config/door", "..").expect("a link inside");

            let mut through = tar::Header::new_gnu();
            through.set_entry_type(tar::EntryType::Symlink);
            through.set_size(0);
            through.set_mode(0o777);
            builder
                .append_link(&mut through, "config/out", "door/../../../etc/passwd")
                .expect("a link through it");
        });

        let into = out.path().join("into");
        let refused = unpack(&archive, &into, &Progress::default()).expect_err("it must refuse");
        assert!(refused.to_string().contains("points out"), "{refused}");
        assert!(!into.join("config/out").exists());
    }

    #[test]
    fn a_cancelled_pack_stops_where_it_stood() {
        let tree = Tree::new("cancel");
        for index in 0..50 {
            tree.file(&format!("mods/mod-{index}.jar"), &[7u8; 1024]);
        }
        let out = Tree::new("cancel-out");
        let archive = out.path().join("backup.tar.zst");

        let plan = survey(tree.path()).expect("a survey");
        let progress = Progress::default();
        progress.cancel();
        let stopped = pack(tree.path(), &plan, &archive, &progress).expect_err("it stops");
        assert!(stopped.downcast_ref::<Cancelled>().is_some(), "{stopped}");
    }

    #[test]
    fn the_free_space_is_the_kernels_answer() {
        let tree = Tree::new("space");
        let free = free_bytes(tree.path()).expect("a number");
        assert!(free > 0, "a writable temporary directory has room");
    }

    #[test]
    fn a_target_is_judged_by_where_it_lands_and_not_by_how_it_is_spelled() {
        let out = Tree::new("landing");
        std::fs::create_dir_all(out.path().join("root/config")).expect("a config directory");
        symlink("..", out.path().join("root/config/door")).expect("a link to the root");
        let root = out.path().join("root").canonicalize().expect("a resolved root");

        let from = |target| confine_link(&root, Path::new("config/keys"), Path::new(target));
        assert!(from("../world/level.dat").is_ok(), "up and back down again stays inside");
        assert!(from("../../etc/passwd").is_err());
        assert!(from("/etc/passwd").is_err(), "an absolute target names another tree");
        assert!(
            from("door/..").is_err(),
            "one hop through a link to the root is one hop out of the tree"
        );
        assert!(from("door/mods/handy.jar").is_ok(), "the same link the other way stays inside");
    }

    #[test]
    fn a_hard_link_in_an_archive_is_refused() {
        let out = Tree::new("hard-link");
        let archive = out.path().join("evil.tar.zst");
        write_tar(&archive, |builder| {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Link);
            header.set_size(0);
            header.set_mode(0o644);
            builder.append_link(&mut header, "config/keys", "/etc/passwd").expect("a hard link");
        });

        let into = out.path().join("into");
        let refused = unpack(&archive, &into, &Progress::default()).expect_err("it must refuse");
        assert!(refused.to_string().contains("hard link"), "{refused}");
        assert!(!into.join("config/keys").exists());
    }

    #[test]
    fn an_archive_without_directory_entries_still_comes_out() {
        let out = Tree::new("no-dirs");
        let archive = out.path().join("flat.tar.zst");
        write_tar(&archive, |builder| {
            let payload = b"level";
            let mut header = tar::Header::new_gnu();
            header.set_size(payload.len() as u64);
            header.set_mode(0o644);
            builder.append_data(&mut header, "world/level.dat", &payload[..]).expect("an entry");
        });

        let into = out.path().join("into");
        unpack(&archive, &into, &Progress::default()).expect("the parents are made on the way");
        assert_eq!(std::fs::read(into.join("world/level.dat")).expect("the file"), b"level");
    }

    #[test]
    fn a_file_that_shrinks_while_it_is_read_does_not_shift_the_entries_after_it() {
        let out = Tree::new("shrinking");
        let archive = out.path().join("shifted.tar.zst");
        write_tar(&archive, |builder| {
            let mut header = tar::Header::new_gnu();
            header.set_size(1_000);
            header.set_mode(0o644);
            builder
                .append_data(
                    &mut header,
                    "world/level.dat",
                    Exactly { inner: &b"half"[..], left: 1_000 },
                )
                .expect("a file that lost weight");

            let payload = b"still here";
            let mut second = tar::Header::new_gnu();
            second.set_size(payload.len() as u64);
            second.set_mode(0o644);
            builder
                .append_data(&mut second, "server.properties", &payload[..])
                .expect("the entry behind it");
        });

        let into = out.path().join("into");
        unpack(&archive, &into, &Progress::default()).expect("the archive is still whole");
        assert_eq!(
            std::fs::read(into.join("server.properties")).expect("the second entry"),
            b"still here"
        );
        assert_eq!(
            std::fs::read(into.join("world/level.dat")).expect("the first entry").len(),
            1_000,
            "the promise in the header is kept, with zeroes if it has to be"
        );
    }

    #[test]
    fn exactly_pads_a_short_file_and_drops_what_a_growing_one_added() {
        use std::io::Read;

        let mut short = Vec::new();
        Exactly { inner: &b"abc"[..], left: 8 }.read_to_end(&mut short).expect("a read");
        assert_eq!(short, b"abc\0\0\0\0\0");

        let mut long = Vec::new();
        Exactly { inner: &b"abcdefghij"[..], left: 4 }.read_to_end(&mut long).expect("a read");
        assert_eq!(long, b"abcd");
    }

    #[test]
    fn a_path_through_links_the_archive_itself_laid_down_stays_under_the_root() {
        let out = Tree::new("through-links");
        let archive = out.path().join("winding.tar.zst");
        write_tar(&archive, |builder| {
            let mut dir = tar::Header::new_gnu();
            dir.set_entry_type(tar::EntryType::Directory);
            dir.set_size(0);
            dir.set_mode(0o755);
            builder.append_data(&mut dir, "deep/", std::io::empty()).expect("a directory");

            let mut up = tar::Header::new_gnu();
            up.set_entry_type(tar::EntryType::Symlink);
            up.set_size(0);
            up.set_mode(0o777);
            builder.append_link(&mut up, "deep/up", "..").expect("a link back up");

            let payload = b"planted";
            let mut file = tar::Header::new_gnu();
            file.set_size(payload.len() as u64);
            file.set_mode(0o644);
            builder
                .append_data(&mut file, "deep/up/planted.txt", &payload[..])
                .expect("a file through the link");
        });

        let into = out.path().join("into");
        unpack(&archive, &into, &Progress::default()).expect("every entry stays inside");
        assert!(into.join("planted.txt").exists(), "it landed in the root, which is inside");
        assert!(
            !out.path().join("planted.txt").exists(),
            "and not one level up, where the link was pointing from"
        );
    }

    #[test]
    fn a_late_entry_that_walks_out_takes_the_whole_run_with_it() {
        let out = Tree::new("late-escape");
        let archive = out.path().join("late.tar.zst");
        write_tar(&archive, |builder| {
            let payload = b"level";
            let mut file = tar::Header::new_gnu();
            file.set_size(payload.len() as u64);
            file.set_mode(0o644);
            builder.append_data(&mut file, "world/level.dat", &payload[..]).expect("an entry");

            let mut escape = tar::Header::new_gnu();
            escape.set_entry_type(tar::EntryType::Symlink);
            escape.set_size(0);
            escape.set_mode(0o777);
            builder.append_link(&mut escape, "escape", "../../elsewhere").expect("a way out");
        });

        let into = out.path().join("nested").join("into");
        let refused = unpack(&archive, &into, &Progress::default()).expect_err("it must refuse");
        assert!(refused.downcast_ref::<Escapes>().is_some(), "{refused}");
        assert!(!into.join("escape").exists());
        assert!(!out.path().join("elsewhere").exists(), "nothing was made outside");
    }

    fn write_tar(archive: &Path, fill: impl FnOnce(&mut tar::Builder<zstd::Encoder<'_, std::fs::File>>)) {
        let file = std::fs::File::create(archive).expect("a file");
        let encoder = zstd::Encoder::new(file, LEVEL).expect("an encoder");
        let mut builder = tar::Builder::new(encoder);
        fill(&mut builder);
        let mut done = builder.into_inner().expect("the encoder back").finish().expect("zstd");
        done.flush().expect("flushed");
    }
}
