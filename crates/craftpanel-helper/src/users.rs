use std::ffi::{CStr, CString, OsStr};
use std::io;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use craftpanel_proto::system_username;

use crate::beneath::{Held, Root};

const DIR_MODE: u32 = 0o2770;
const FILE_MODE: u32 = 0o0660;

const DIRECTORY: libc::c_int =
    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;

const MAX_DEPTH: usize = 256;

pub struct Account {
    pub uid: u32,
    pub gid: u32,
}

pub struct Created {
    pub uid: u32,
    pub gid: u32,
    pub home: PathBuf,
}

pub fn lookup(username: &str) -> Result<Option<Account>> {
    let passwd = std::fs::read_to_string("/etc/passwd").context("reading /etc/passwd")?;
    for line in passwd.lines() {
        let mut fields = line.split(':');
        if fields.next() != Some(username) {
            continue;
        }
        let mut fields = fields.skip(1);
        let uid = fields.next().and_then(|v| v.parse().ok());
        let gid = fields.next().and_then(|v| v.parse().ok());
        return match (uid, gid) {
            (Some(uid), Some(gid)) => Ok(Some(Account { uid, gid })),
            _ => bail!("malformed passwd entry for {username}"),
        };
    }
    Ok(None)
}

pub fn group_id(group: &str) -> Result<u32> {
    let content = std::fs::read_to_string("/etc/group").context("reading /etc/group")?;
    for line in content.lines() {
        let mut fields = line.split(':');
        if fields.next() != Some(group) {
            continue;
        }
        if let Some(gid) = fields.nth(1).and_then(|v| v.parse().ok()) {
            return Ok(gid);
        }
    }
    bail!("group {group} does not exist")
}

pub fn chown_to_group(path: &Path, group: &str) -> Result<()> {
    let gid = group_id(group)?;
    let raw = cstring(path.as_os_str().as_bytes())?;
    let done = unsafe {
        libc::fchownat(libc::AT_FDCWD, raw.as_ptr(), u32::MAX, gid, libc::AT_SYMLINK_NOFOLLOW)
    };
    if done != 0 {
        bail!("chown {} failed: {}", path.display(), io::Error::last_os_error());
    }
    Ok(())
}

pub fn create(users: &Root, user_id: &str, shared_group: &str) -> Result<Created> {
    let username = system_username(user_id);
    let home = users.path().join(user_id);
    let shared_gid = group_id(shared_group)?;

    if lookup(&username)?.is_none() {
        let status = Command::new("useradd")
            .arg("--home-dir")
            .arg(&home)
            .arg("--no-create-home")
            .arg("--shell")
            .arg("/usr/sbin/nologin")
            .arg("--comment")
            .arg("craftpanel managed account")
            .arg(&username)
            .status()
            .context("running useradd")?;
        if !status.success() {
            bail!("useradd for {username} exited with {status}");
        }
    }

    let account = lookup(&username)?.context("account missing right after useradd")?;

    let home_fd = users
        .make_home(user_id, DIR_MODE)
        .with_context(|| format!("creating {}", home.display()))?;
    let servers = home.join(craftpanel_proto::SERVERS);
    let servers_name = cstring(craftpanel_proto::SERVERS.as_bytes())?;
    make_dir(home_fd.as_fd(), &servers_name)
        .with_context(|| format!("creating {}", servers.display()))?;
    let servers_fd = open_child(home_fd.as_fd(), &servers_name)
        .with_context(|| format!("opening {}", servers.display()))?;

    for (fd, dir) in [(home_fd.as_fd(), &home), (servers_fd.as_fd(), &servers)] {
        set_owner(fd, account.uid, shared_gid)
            .with_context(|| format!("chown {}", dir.display()))?;
        set_mode(fd, DIR_MODE)
            .with_context(|| format!("setting mode on {}", dir.display()))?;
    }

    Ok(Created { uid: account.uid, gid: account.gid, home })
}

pub fn chown_tree(target: Held, uid: u32, shared_group: &str) -> Result<u64> {
    let gid = group_id(shared_group)?;
    let root = target.path().to_path_buf();
    say_when_second_names_are_free();

    if !target.is_dir().with_context(|| format!("looking at {}", root.display()))? {
        let (_, names) =
            facts_of(target.as_fd()).with_context(|| format!("looking at {}", root.display()))?;
        if names > 1 {
            bail!(
                "{} has more than one name ({names}), so a chown through it would land on an \
                 inode nobody ever gave this account",
                root.display()
            );
        }
        set_owner(target.as_fd(), uid, gid)
            .with_context(|| format!("chown {}", root.display()))?;
        set_mode(target.as_fd(), FILE_MODE)
            .with_context(|| format!("setting mode on {}", root.display()))?;
        return Ok(1);
    }

    let fd = target.into_fd();
    set_owner(fd.as_fd(), uid, gid).with_context(|| format!("chown {}", root.display()))?;
    set_mode(fd.as_fd(), DIR_MODE)
        .with_context(|| format!("setting mode on {}", root.display()))?;
    let mut touched = 1;

    let names = read_dir(fd.as_fd()).with_context(|| format!("listing {}", root.display()))?;
    let mut stack = vec![Level { fd, path: root, names: names.into_iter() }];
    let mut shared = Shared::default();

    while let Some(level) = stack.last_mut() {
        let Some(name) = level.names.next() else {
            stack.pop();
            continue;
        };

        let here = level.path.join(OsStr::from_bytes(name.to_bytes()));
        let step = visit(level.fd.as_fd(), &name, uid, gid, &here)?;

        match step {
            Step::Skipped => {}
            Step::Shared(names) => shared.note(here, names),
            Step::Touched => touched += 1,
            Step::Down(fd) => {
                touched += 1;
                if stack.len() >= MAX_DEPTH {
                    bail!("{} is nested deeper than {MAX_DEPTH} directories", here.display());
                }
                let names =
                    read_dir(fd.as_fd()).with_context(|| format!("listing {}", here.display()))?;
                stack.push(Level { fd, path: here, names: names.into_iter() });
            }
        }
    }

    shared.say();
    Ok(touched)
}

struct Level {
    fd: OwnedFd,
    path: PathBuf,
    names: std::vec::IntoIter<CString>,
}

enum Step {
    Skipped,
    Touched,
    Shared(u64),
    Down(OwnedFd),
}

#[derive(Default)]
struct Shared {
    count: u64,
    first: Vec<(PathBuf, u64)>,
}

impl Shared {
    fn note(&mut self, path: PathBuf, names: u64) {
        self.count += 1;
        if self.first.len() < 8 {
            self.first.push((path, names));
        }
    }

    fn line(&self) -> Option<String> {
        if self.count == 0 {
            return None;
        }
        let named: Vec<String> = self
            .first
            .iter()
            .map(|(path, names)| format!("{} ({names} names)", path.display()))
            .collect();
        Some(format!(
            "{} file(s) left exactly as they were: an inode with a second name has one name in \
             this tree and one that may be anywhere on this machine, and a chown reaches both. {}",
            self.count,
            named.join(", ")
        ))
    }

    fn say(&self) {
        if let Some(line) = self.line() {
            tracing::warn!(count = self.count, "{line}");
        }
    }
}

const PROTECTED_HARDLINKS: &str = "/proc/sys/fs/protected_hardlinks";

fn say_when_second_names_are_free() {
    static SAID: std::sync::Once = std::sync::Once::new();
    SAID.call_once(|| {
        let Ok(raw) = std::fs::read_to_string(PROTECTED_HARDLINKS) else { return };
        if !second_names_are_guarded(&raw) {
            tracing::warn!(
                "{PROTECTED_HARDLINKS} is 0: an account may give a second name to a file it \
                 neither owns nor may read. Nothing here follows one, but set it to 1"
            );
        }
    });
}

fn second_names_are_guarded(raw: &str) -> bool {
    raw.trim() != "0"
}

fn visit(dir: BorrowedFd<'_>, name: &CStr, uid: u32, gid: u32, here: &Path) -> Result<Step> {
    let kind = match kind_of(dir, name) {
        Ok(kind) => kind,
        Err(err) if moved(&err) => return Ok(Step::Skipped),
        Err(err) => bail!("reading {}: {err}", here.display()),
    };

    if kind == libc::S_IFLNK {
        return Ok(Step::Skipped);
    }

    if kind == libc::S_IFDIR {
        let child = match open_child(dir, name) {
            Ok(child) => child,
            Err(err) if moved(&err) => return Ok(Step::Skipped),
            Err(err) => bail!("opening {}: {err}", here.display()),
        };
        set_owner(child.as_fd(), uid, gid).with_context(|| format!("chown {}", here.display()))?;
        set_mode(child.as_fd(), DIR_MODE)
            .with_context(|| format!("setting mode on {}", here.display()))?;
        return Ok(Step::Down(child));
    }

    match hand_back_file(dir, name, uid, gid) {
        Ok(Handed::Back) => Ok(Step::Touched),
        Ok(Handed::Shared(names)) => Ok(Step::Shared(names)),
        Ok(Handed::Passed) => Ok(Step::Skipped),
        Err(err) => Err(anyhow::Error::new(err))
            .with_context(|| format!("handing back {}", here.display())),
    }
}

fn hand_back_file(dir: BorrowedFd<'_>, name: &CStr, uid: u32, gid: u32) -> io::Result<Handed> {
    let fd = match open_entry(dir, name) {
        Ok(fd) => fd,
        Err(err) => {
            return match err.raw_os_error() {
                Some(libc::ENOENT) | Some(libc::ELOOP) | Some(libc::ENXIO) => Ok(Handed::Passed),
                _ => Err(err),
            }
        }
    };

    let (kind, names) = facts_of(fd.as_fd())?;
    if kind != libc::S_IFREG {
        return Ok(Handed::Passed);
    }
    if names > 1 {
        return Ok(Handed::Shared(names));
    }

    set_owner(fd.as_fd(), uid, gid)?;
    set_mode(fd.as_fd(), FILE_MODE)?;
    Ok(Handed::Back)
}

enum Handed {
    Back,
    Shared(u64),
    Passed,
}

fn open_entry(dir: BorrowedFd<'_>, name: &CStr) -> io::Result<OwnedFd> {
    let flags =
        libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_NOCTTY | libc::O_CLOEXEC;
    take_fd(unsafe { libc::openat(dir.as_raw_fd(), name.as_ptr(), flags) })
}

fn facts_of(fd: BorrowedFd<'_>) -> io::Result<(u32, u64)> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    check(unsafe { libc::fstat(fd.as_raw_fd(), stat.as_mut_ptr()) })?;
    let stat = unsafe { stat.assume_init() };
    Ok((stat.st_mode & libc::S_IFMT, stat.st_nlink as u64))
}

fn moved(err: &io::Error) -> bool {
    matches!(err.raw_os_error(), Some(libc::ENOENT) | Some(libc::ENOTDIR))
}

#[cfg(test)]
fn own_group() -> Option<String> {
    let gid = unsafe { libc::getgid() };
    let content = std::fs::read_to_string("/etc/group").ok()?;
    content.lines().find_map(|line| {
        let mut fields = line.split(':');
        let name = fields.next()?;
        (fields.nth(1)? == gid.to_string()).then(|| name.to_owned())
    })
}

pub fn delete(user_id: &str, remove_home: bool) -> Result<()> {
    let username = system_username(user_id);
    if lookup(&username)?.is_none() {
        return Ok(());
    }

    let mut command = Command::new("userdel");
    if remove_home {
        command.arg("--remove");
    }
    let status = command.arg(&username).status().context("running userdel")?;
    if !status.success() {
        bail!("userdel for {username} exited with {status}");
    }
    Ok(())
}

fn open_child(dir: BorrowedFd<'_>, name: &CStr) -> io::Result<OwnedFd> {
    take_fd(unsafe { libc::openat(dir.as_raw_fd(), name.as_ptr(), DIRECTORY) })
}

fn make_dir(dir: BorrowedFd<'_>, name: &CStr) -> io::Result<()> {
    match check(unsafe { libc::mkdirat(dir.as_raw_fd(), name.as_ptr(), DIR_MODE) }) {
        Err(err) if err.raw_os_error() == Some(libc::EEXIST) => Ok(()),
        other => other,
    }
}

fn set_owner(fd: BorrowedFd<'_>, uid: u32, gid: u32) -> io::Result<()> {
    check(unsafe { libc::fchown(fd.as_raw_fd(), uid, gid) })
}

fn set_mode(fd: BorrowedFd<'_>, mode: u32) -> io::Result<()> {
    check(unsafe { libc::fchmod(fd.as_raw_fd(), mode) })
}

fn kind_of(dir: BorrowedFd<'_>, name: &CStr) -> io::Result<u32> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    let done = unsafe {
        libc::fstatat(dir.as_raw_fd(), name.as_ptr(), stat.as_mut_ptr(), libc::AT_SYMLINK_NOFOLLOW)
    };
    check(done)?;
    Ok(unsafe { stat.assume_init() }.st_mode & libc::S_IFMT)
}

#[repr(C)]
struct Dirent {
    inode: u64,
    offset: i64,
    length: u16,
    kind: u8,
}

fn read_dir(fd: BorrowedFd<'_>) -> io::Result<Vec<CString>> {
    let mut buffer = vec![0u64; 4096];
    let bytes = buffer.len() * std::mem::size_of::<u64>();
    let mut names = Vec::new();

    loop {
        let read = unsafe {
            libc::syscall(libc::SYS_getdents64, fd.as_raw_fd(), buffer.as_mut_ptr(), bytes)
        };
        if read < 0 {
            return Err(io::Error::last_os_error());
        }
        if read == 0 {
            return Ok(names);
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
                names.push(cstring(name)?);
            }
            at += length;
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let mine = std::process::id();
            let dir = std::env::temp_dir().join(format!("craftpanel-users-{name}-{mine}"));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = Command::new("chmod").arg("-R").arg("u+rwX").arg(&self.0).status();
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn mode_of(path: &Path) -> u32 {
        std::fs::symlink_metadata(path).expect("the entry").permissions().mode() & 0o7777
    }

    fn owner_of(path: &Path) -> (u32, u32) {
        let meta = std::fs::symlink_metadata(path).expect("the entry");
        (std::os::unix::fs::MetadataExt::uid(&meta), std::os::unix::fs::MetadataExt::gid(&meta))
    }

    const STRANGER: u32 = 61234;

    fn plant(path: &Path) -> ((u32, u32), u32) {
        std::fs::write(path, b"someone else's").unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let _ = std::os::unix::fs::lchown(path, Some(STRANGER), Some(STRANGER));
        (owner_of(path), mode_of(path))
    }

    fn mine() -> u32 {
        unsafe { libc::getuid() }
    }

    fn way_in(scratch: &Path, steps: &[&str]) -> Held {
        let users = scratch.parent().expect("the scratch sits somewhere");
        let account = scratch.file_name().and_then(|name| name.to_str()).expect("a plain name");
        let steps: Vec<String> = steps.iter().map(|step| (*step).to_owned()).collect();
        crate::beneath::Root::open(users)
            .expect("the users directory opens")
            .home(account)
            .expect("the account's own directory")
            .entry(&steps)
            .expect("the steps lead somewhere")
    }

    #[test]
    fn a_tree_the_game_locked_comes_back_with_the_group_let_in() {
        let Some(group) = own_group() else {
            eprintln!("skipped: this process has no group of its own to hand a file to");
            return;
        };
        let scratch = Scratch::new("locked");
        let locked = scratch.0.join("plugins/WorldEdit/.archive-unpack/0ac1a273/lang");
        std::fs::create_dir_all(&locked).unwrap();
        let file = locked.join("strings.json");
        std::fs::write(&file, b"{}").unwrap();
        let dirs = [scratch.0.as_path(), locked.parent().unwrap(), locked.as_path()];
        for dir in dirs {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o2700)).unwrap();
        }
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();

        let touched = chown_tree(way_in(&scratch.0, &[]), mine(), &group).unwrap();

        assert_eq!(touched, 7, "six directories and the file, none skipped");
        for dir in dirs {
            assert_eq!(mode_of(dir), 0o2770, "{} still keeps the panel out", dir.display());
        }
        assert_eq!(mode_of(&file), 0o660);
    }

    #[test]
    fn a_symlink_is_walked_past_and_not_through() {
        let Some(group) = own_group() else {
            eprintln!("skipped: this process has no group of its own to hand a file to");
            return;
        };
        let scratch = Scratch::new("symlink");
        let outside = scratch.0.join("outside.txt");
        let before = plant(&outside);
        let inside = scratch.0.join("tree");
        std::fs::create_dir_all(&inside).unwrap();
        std::os::unix::fs::symlink(&outside, inside.join("link")).unwrap();

        let touched = chown_tree(way_in(&scratch.0, &["tree"]), mine(), &group).unwrap();

        assert_eq!(touched, 1, "the directory, and the link counts for nothing");
        assert_eq!(
            (owner_of(&outside), mode_of(&outside)),
            before,
            "what the link points at kept its owner and its mode"
        );
    }

    #[test]
    fn a_link_to_a_directory_outside_is_not_walked_into() {
        let Some(group) = own_group() else {
            eprintln!("skipped: this process has no group of its own to hand a file to");
            return;
        };
        let scratch = Scratch::new("link-to-dir");
        let outside = scratch.0.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let treasure = outside.join("panel.db");
        let before = plant(&treasure);
        std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o700)).unwrap();

        let inside = scratch.0.join("tree");
        std::fs::create_dir_all(inside.join("plugins")).unwrap();
        std::os::unix::fs::symlink(&outside, inside.join("plugins/shortcut")).unwrap();

        let touched = chown_tree(way_in(&scratch.0, &["tree"]), mine(), &group).unwrap();

        assert_eq!(touched, 2, "the tree and its plugins directory, and nothing beyond the link");
        assert_eq!((owner_of(&treasure), mode_of(&treasure)), before);
        assert_eq!(mode_of(&outside), 0o700, "the directory on the far end is untouched too");
    }

    #[test]
    fn a_file_with_a_second_name_is_left_where_it_is() {
        let Some(group) = own_group() else {
            eprintln!("skipped: this process has no group of its own to hand a file to");
            return;
        };
        let scratch = Scratch::new("hardlink");
        let outside = scratch.0.join("outside.txt");
        let before = plant(&outside);

        let inside = scratch.0.join("tree");
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::hard_link(&outside, inside.join("loot")).unwrap();
        let ordinary = inside.join("plugin.yml");
        std::fs::write(&ordinary, b"x").unwrap();
        std::fs::set_permissions(&ordinary, std::fs::Permissions::from_mode(0o600)).unwrap();

        let touched = chown_tree(way_in(&scratch.0, &["tree"]), mine(), &group).unwrap();

        assert_eq!(
            (owner_of(&outside), mode_of(&outside)),
            before,
            "the file the second name shares kept its owner and its mode"
        );
        assert_eq!(touched, 2, "the directory and the one file that has a single name");
        assert_eq!(mode_of(&ordinary), 0o660, "the walk carried on past it");
    }

    #[test]
    fn the_act_on_a_file_stops_at_a_name_that_has_become_a_second_name() {
        let scratch = Scratch::new("swapped-hardlink");
        let outside = scratch.0.join("outside.txt");
        let before = plant(&outside);

        let inside = scratch.0.join("tree");
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::write(inside.join("config.yml"), b"a: 1\n").unwrap();
        let dir = way_in(&scratch.0, &["tree"]);

        std::fs::remove_file(inside.join("config.yml")).unwrap();
        std::fs::hard_link(&outside, inside.join("config.yml")).unwrap();

        let handed = hand_back_file(dir.as_fd(), c"config.yml", mine(), 0).unwrap();

        assert_eq!(
            (owner_of(&outside), mode_of(&outside)),
            before,
            "the file the second name shares kept its owner and its mode"
        );
        assert!(matches!(handed, Handed::Shared(2)), "and the act says why it did nothing");
    }

    #[test]
    fn what_the_walk_left_alone_goes_out_by_name() {
        let mut shared = Shared::default();
        assert!(shared.line().is_none(), "an ordinary tree says nothing");

        shared.note(PathBuf::from("/var/lib/craftpanel/users/01/servers/s/loot"), 2);
        let line = shared.line().expect("a line");

        assert!(line.contains("servers/s/loot"), "{line}");
        assert!(line.contains("(2 names)"), "{line}");
        assert!(line.starts_with('1'), "{line}");
    }

    #[test]
    fn the_setting_that_lets_an_account_name_a_stranger_is_read_as_written() {
        assert!(!second_names_are_guarded("0\n"));
        assert!(second_names_are_guarded("1\n"));
        assert!(second_names_are_guarded(""));
    }

    #[test]
    fn a_lone_file_with_a_second_name_is_refused_rather_than_handed_back() {
        let Some(group) = own_group() else {
            eprintln!("skipped: this process has no group of its own to hand a file to");
            return;
        };
        let scratch = Scratch::new("hardlink-alone");
        let outside = scratch.0.join("outside.txt");
        let before = plant(&outside);

        let inside = scratch.0.join("tree");
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::hard_link(&outside, inside.join("loot")).unwrap();

        let refused = chown_tree(way_in(&scratch.0, &["tree", "loot"]), mine(), &group)
            .expect_err("this must not go through");

        assert!(format!("{refused:#}").contains("more than one name"), "{refused:#}");
        assert_eq!((owner_of(&outside), mode_of(&outside)), before);
    }

    #[test]
    fn the_act_on_a_file_stops_at_a_name_that_has_become_a_link() {
        let scratch = Scratch::new("swapped-file");
        let outside = scratch.0.join("outside.txt");
        let before = plant(&outside);

        let inside = scratch.0.join("tree");
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::write(inside.join("config.yml"), b"a: 1\n").unwrap();
        let dir = way_in(&scratch.0, &["tree"]);

        std::fs::remove_file(inside.join("config.yml")).unwrap();
        std::os::unix::fs::symlink(&outside, inside.join("config.yml")).unwrap();

        let handed = hand_back_file(dir.as_fd(), c"config.yml", mine(), 0).unwrap();

        assert_eq!(
            (owner_of(&outside), mode_of(&outside)),
            before,
            "the file outside the tree kept its owner and its mode"
        );
        assert!(
            matches!(handed, Handed::Passed),
            "the act says it found a link rather than doing anything to it"
        );
    }

    #[test]
    fn the_step_into_a_directory_stops_at_a_name_that_has_become_a_link() {
        let scratch = Scratch::new("swapped-dir");
        let outside = scratch.0.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let inside = scratch.0.join("tree");
        std::fs::create_dir_all(inside.join("plugins")).unwrap();
        let dir = way_in(&scratch.0, &["tree"]);

        std::fs::remove_dir(inside.join("plugins")).unwrap();
        std::os::unix::fs::symlink(&outside, inside.join("plugins")).unwrap();

        let refused = open_child(dir.as_fd(), c"plugins").expect_err("no descriptor of that");
        assert_eq!(refused.raw_os_error(), Some(libc::ENOTDIR));
        assert!(moved(&refused), "which the walk reads as the tree moving and walks on");
    }

    #[test]
    fn an_entry_that_went_away_under_the_walk_is_not_a_failure() {
        let scratch = Scratch::new("vanished");
        let dir = way_in(&scratch.0, &[]);
        let gone = scratch.0.join("r.0.0.mca");

        let step = visit(dir.as_fd(), c"r.0.0.mca", mine(), 0, &gone).expect("no error");

        assert!(matches!(step, Step::Skipped));
    }

    fn nest(root: &Path, levels: usize) -> PathBuf {
        let mut path = root.to_path_buf();
        for _ in 0..levels {
            path.push("d");
        }
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn open_descriptors() -> usize {
        std::fs::read_dir("/proc/self/fd").map(Iterator::count).unwrap_or(0)
    }

    #[test]
    fn a_tree_two_hundred_deep_comes_back_whole() {
        let Some(group) = own_group() else {
            eprintln!("skipped: this process has no group of its own to hand a file to");
            return;
        };
        let scratch = Scratch::new("deepish");
        let bottom = nest(&scratch.0, 200);
        let file = bottom.join("level.dat");
        std::fs::write(&file, b"x").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();

        let touched = chown_tree(way_in(&scratch.0, &[]), mine(), &group).unwrap();

        assert_eq!(touched, 202, "the root, two hundred levels and the file at the bottom");
        assert_eq!(mode_of(&bottom), 0o2770);
        assert_eq!(mode_of(&file), 0o660);
    }

    #[test]
    fn a_tree_deeper_than_the_walk_allows_is_refused_and_lets_go_of_everything() {
        let Some(group) = own_group() else {
            eprintln!("skipped: this process has no group of its own to hand a file to");
            return;
        };
        let scratch = Scratch::new("deep");
        nest(&scratch.0, MAX_DEPTH + 8);
        let before = open_descriptors();

        for _ in 0..20 {
            let err = chown_tree(way_in(&scratch.0, &[]), mine(), &group)
                .expect_err("this must not go through");
            assert!(format!("{err:#}").contains("nested deeper"), "{err:#}");
        }

        assert!(
            open_descriptors() < before + 64,
            "{before} descriptors open before twenty refused walks, {} after",
            open_descriptors()
        );
    }

    fn exchange(dir: BorrowedFd<'_>, one: &CStr, other: &CStr) -> bool {
        let done = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                dir.as_raw_fd(),
                one.as_ptr(),
                dir.as_raw_fd(),
                other.as_ptr(),
                libc::RENAME_EXCHANGE,
            )
        };
        done == 0
    }

    #[test]
    fn a_swap_under_a_running_walk_never_reaches_out_of_the_tree() {
        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
        use std::sync::Arc;

        let Some(group) = own_group() else {
            eprintln!("skipped: this process has no group of its own to hand a file to");
            return;
        };
        let scratch = Scratch::new("race");
        let outside = scratch.0.join("outside.txt");
        let file_before = plant(&outside);
        let elsewhere = scratch.0.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let treasure = elsewhere.join("panel.db");
        let treasure_before = plant(&treasure);
        std::fs::set_permissions(&elsewhere, std::fs::Permissions::from_mode(0o700)).unwrap();

        let inside = scratch.0.join("tree");
        std::fs::create_dir_all(inside.join("pad")).unwrap();
        for index in 0..500 {
            std::fs::write(inside.join(format!("pad/r.{index}.mca")), b"x").unwrap();
        }
        let mut names = Vec::new();
        for index in 0..256 {
            let bait = inside.join(format!("bait{index}"));
            let link = inside.join(format!("link{index}"));
            if index % 2 == 0 {
                std::fs::create_dir_all(&bait).unwrap();
                std::fs::write(bait.join("plugin.yml"), b"x").unwrap();
                std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
            } else {
                std::fs::write(&bait, b"x").unwrap();
                std::os::unix::fs::symlink(&outside, &link).unwrap();
            }
            names.push((
                CString::new(format!("bait{index}")).unwrap(),
                CString::new(format!("link{index}")).unwrap(),
            ));
        }

        let stop = Arc::new(AtomicBool::new(false));
        let swaps = Arc::new(AtomicU64::new(0));
        let swappers: Vec<_> = (0..4)
            .map(|which| {
                let (flag, counted) = (stop.clone(), swaps.clone());
                let pairs: Vec<_> = names
                    .iter()
                    .skip(which)
                    .step_by(4)
                    .map(|(bait, link)| (bait.clone(), link.clone()))
                    .collect();
                let tree = way_in(&scratch.0, &["tree"]);
                std::thread::spawn(move || {
                    while !flag.load(Ordering::Relaxed) {
                        for (bait, link) in &pairs {
                            if exchange(tree.as_fd(), bait, link) {
                                counted.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                })
            })
            .collect();
        while swaps.load(Ordering::Relaxed) < 256 {
            std::thread::yield_now();
        }

        let mut walked = Vec::new();
        for _ in 0..8 {
            walked.push(chown_tree(way_in(&scratch.0, &["tree"]), mine(), &group));
        }
        stop.store(true, Ordering::Relaxed);
        for swapper in swappers {
            swapper.join().unwrap();
        }

        for walk in walked {
            walk.expect("a tree that moves under the walk is not a failure");
        }
        assert_eq!(
            (owner_of(&outside), mode_of(&outside)),
            file_before,
            "the file outside the tree kept its owner and its mode"
        );
        assert_eq!(
            (owner_of(&treasure), mode_of(&treasure)),
            treasure_before,
            "and so did what lies in the directory outside the tree"
        );
        assert_eq!(mode_of(&elsewhere), 0o700, "which was never stepped into");
    }
}
