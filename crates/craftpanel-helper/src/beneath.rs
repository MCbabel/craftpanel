use std::ffi::CString;
use std::io;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use craftpanel_proto::is_valid_step;

const RESOLVE_NO_MAGICLINKS: u64 = 0x02;
const RESOLVE_BENEATH: u64 = 0x08;

const INSIDE: u64 = RESOLVE_BENEATH | RESOLVE_NO_MAGICLINKS;

pub const ROOT_MODE: u32 = 0o751;

const DIRECTORY: i32 = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC;

const ANYTHING: i32 = libc::O_RDONLY | libc::O_NONBLOCK | libc::O_CLOEXEC;

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
pub struct Home {
    fd: OwnedFd,
    path: PathBuf,
}

#[derive(Debug)]
pub struct Held {
    fd: OwnedFd,
    path: PathBuf,
}

impl Root {
    pub fn prepare(path: &Path, gid: u32) -> io::Result<(Self, bool)> {
        let raw = cstring(path.as_os_str().as_bytes())?;
        match check(unsafe { libc::mkdir(raw.as_ptr(), ROOT_MODE) }) {
            Err(err) if err.raw_os_error() == Some(libc::EEXIST) => {}
            other => other?,
        }

        let root = Self::open(path)?;
        let stat = fstat(root.fd.as_fd())?;
        let wrong = stat.st_uid != 0 || stat.st_gid != gid || stat.st_mode & 0o7777 != ROOT_MODE;
        if wrong {
            check(unsafe { libc::fchown(root.fd.as_raw_fd(), 0, gid) })?;
            check(unsafe { libc::fchmod(root.fd.as_raw_fd(), ROOT_MODE) })?;
        }

        Ok((root, wrong))
    }

    pub fn open(path: &Path) -> io::Result<Self> {
        let raw = cstring(path.as_os_str().as_bytes())?;
        let fd = take_fd(unsafe { libc::open(raw.as_ptr(), DIRECTORY | libc::O_NOFOLLOW) })?;
        Ok(Self { fd, path: path.to_path_buf() })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn home(&self, user_id: &str) -> io::Result<Home> {
        if !is_valid_step(user_id) {
            return Err(io::Error::from_raw_os_error(libc::EINVAL));
        }
        let fd = openat(self.fd.as_fd(), user_id.as_bytes(), DIRECTORY | libc::O_NOFOLLOW, 0)?;
        Ok(Home { fd, path: self.path.join(user_id) })
    }

    pub fn make_home(&self, user_id: &str, mode: u32) -> io::Result<Home> {
        if !is_valid_step(user_id) {
            return Err(io::Error::from_raw_os_error(libc::EINVAL));
        }
        let name = cstring(user_id.as_bytes())?;
        match check(unsafe { libc::mkdirat(self.fd.as_raw_fd(), name.as_ptr(), mode) }) {
            Err(err) if err.raw_os_error() == Some(libc::EEXIST) => {}
            other => other?,
        }
        self.home(user_id)
    }
}

impl Home {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }

    pub fn entry(&self, steps: &[String]) -> io::Result<Held> {
        self.resolve(steps, ANYTHING)
    }

    pub fn dir(&self, steps: &[String]) -> io::Result<Held> {
        self.resolve(steps, DIRECTORY)
    }

    fn resolve(&self, steps: &[String], flags: i32) -> io::Result<Held> {
        if !steps.iter().all(|step| is_valid_step(step)) {
            return Err(io::Error::from_raw_os_error(libc::EINVAL));
        }

        let mut path = self.path.clone();
        for step in steps {
            path.push(step);
        }
        if steps.is_empty() {
            return Ok(Held { fd: self.fd.try_clone()?, path });
        }

        let fd = if openat2_works() {
            match openat2(self.fd.as_fd(), &steps.join("/"), flags, INSIDE) {
                Err(err) if err.raw_os_error() == Some(libc::ENOSYS) => {
                    openat2_gone();
                    step_in(self.fd.as_fd(), steps, flags)?
                }
                other => other?,
            }
        } else {
            step_in(self.fd.as_fd(), steps, flags)?
        };
        Ok(Held { fd, path })
    }
}

impl Held {
    pub fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }

    pub fn into_fd(self) -> OwnedFd {
        self.fd
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_dir(&self) -> io::Result<bool> {
        Ok(fstat(self.fd.as_fd())?.st_mode & libc::S_IFMT == libc::S_IFDIR)
    }
}

fn step_in(base: BorrowedFd<'_>, steps: &[String], flags: i32) -> io::Result<OwnedFd> {
    let (last, leading) = steps.split_last().expect("resolve answers an empty walk itself");
    let mut here = base.try_clone_to_owned()?;
    for step in leading {
        here = openat(here.as_fd(), step.as_bytes(), DIRECTORY | libc::O_NOFOLLOW, 0)?;
    }
    openat(here.as_fd(), last.as_bytes(), flags | libc::O_NOFOLLOW, 0)
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
        tracing::warn!("this kernel has no openat2; falling back to one openat per step");
    }
}

fn openat(dir: BorrowedFd<'_>, name: &[u8], flags: i32, mode: u32) -> io::Result<OwnedFd> {
    let name = cstring(name)?;
    take_fd(unsafe { libc::openat(dir.as_raw_fd(), name.as_ptr(), flags, mode as libc::c_uint) })
}

fn fstat(fd: BorrowedFd<'_>) -> io::Result<libc::stat> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    check(unsafe { libc::fstat(fd.as_raw_fd(), stat.as_mut_ptr()) })?;
    Ok(unsafe { stat.assume_init() })
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
            let dir = std::env::temp_dir()
                .join(format!("craftpanel-beneath-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn users(&self) -> PathBuf {
            self.0.join("users")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn steps(raw: &[&str]) -> Vec<String> {
        raw.iter().map(|step| (*step).to_owned()).collect()
    }

    fn mode_of(path: &Path) -> u32 {
        std::fs::symlink_metadata(path).expect("the entry").permissions().mode() & 0o7777
    }

    fn owner_of(path: &Path) -> (u32, u32) {
        let meta = std::fs::symlink_metadata(path).expect("the entry");
        (std::os::unix::fs::MetadataExt::uid(&meta), std::os::unix::fs::MetadataExt::gid(&meta))
    }

    fn root_only(what: &str) -> bool {
        if unsafe { libc::geteuid() } == 0 {
            return true;
        }
        eprintln!("skipped: {what} needs root");
        false
    }

    const ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

    fn planted(scratch: &Scratch) -> Root {
        std::fs::create_dir_all(scratch.users().join(ID).join("servers/one/plugins")).unwrap();
        std::fs::write(scratch.users().join(ID).join("servers/one/plugins/a.yml"), b"a: 1\n")
            .unwrap();
        Root::open(&scratch.users()).expect("a root")
    }

    #[test]
    fn the_users_directory_is_put_into_roots_hands_at_start_up() {
        if !root_only("owning a directory as root") {
            return;
        }
        let scratch = Scratch::new("prepare");
        std::fs::create_dir_all(scratch.users()).unwrap();
        std::fs::set_permissions(&scratch.users(), std::fs::Permissions::from_mode(0o700)).unwrap();
        std::os::unix::fs::lchown(scratch.users(), Some(61234), Some(61234)).unwrap();

        let (root, repaired) = Root::prepare(&scratch.users(), 0).expect("a root");

        assert!(repaired, "the helper says it had to put the directory right");
        assert_eq!(owner_of(root.path()), (0, 0), "only root may bind a name in here");
        assert_eq!(mode_of(root.path()), ROOT_MODE);
        assert!(!Root::prepare(&scratch.users(), 0).expect("a root").1, "and only says it once");
    }

    fn refused_a_link(err: &io::Error, behind: &Path) {
        assert!(
            matches!(err.raw_os_error(), Some(libc::ELOOP) | Some(libc::ENOTDIR)),
            "the link has to be refused, not followed: {err}"
        );
        assert!(
            std::fs::File::open(behind).is_ok(),
            "and it does lead to a directory this process could have opened"
        );
    }

    #[test]
    fn a_users_directory_that_is_a_link_is_refused_rather_than_followed() {
        let scratch = Scratch::new("root-link");
        std::fs::create_dir_all(scratch.0.join("elsewhere")).unwrap();
        std::os::unix::fs::symlink(scratch.0.join("elsewhere"), scratch.users()).unwrap();

        let refused = Root::prepare(&scratch.users(), 0).expect_err("no root out of a link");
        refused_a_link(&refused, &scratch.0.join("elsewhere"));
    }

    #[test]
    fn an_account_directory_that_is_a_link_is_refused_the_same_way() {
        let scratch = Scratch::new("home-link");
        let root = planted(&scratch);
        std::fs::rename(scratch.users().join(ID), scratch.0.join("moved")).unwrap();
        std::os::unix::fs::symlink(scratch.0.join("moved"), scratch.users().join(ID)).unwrap();

        let refused = root.home(ID).expect_err("no home out of a link");
        refused_a_link(&refused, &scratch.0.join("moved"));
    }

    #[test]
    fn steps_that_are_not_names_never_reach_the_kernel() {
        let scratch = Scratch::new("steps");
        let home = planted(&scratch).home(ID).expect("the home");

        for bad in [vec![".."], vec!["servers", ".."], vec!["/etc"], vec!["servers/one"], vec![""]] {
            let refused = home.entry(&steps(&bad)).expect_err(&format!("{bad:?} must not open"));
            assert_eq!(refused.raw_os_error(), Some(libc::EINVAL), "{bad:?}");
        }
    }

    #[test]
    fn a_link_that_leaves_the_account_is_stopped_by_the_kernel() {
        let scratch = Scratch::new("escape");
        let root = planted(&scratch);
        std::fs::create_dir_all(scratch.0.join("outside")).unwrap();
        std::fs::write(scratch.0.join("outside/loot"), b"x").unwrap();
        std::os::unix::fs::symlink(
            scratch.0.join("outside"),
            scratch.users().join(ID).join("servers/away"),
        )
        .unwrap();

        let home = root.home(ID).expect("the home");
        let refused = home.entry(&steps(&["servers", "away"])).expect_err("that leaves the tree");
        assert!(
            matches!(refused.raw_os_error(), Some(libc::EXDEV) | Some(libc::ELOOP)),
            "the kernel has to stop the walk, not us afterwards: {refused}"
        );
        assert!(home.entry(&steps(&["servers", "one"])).is_ok(), "and what stays inside opens");
    }

    #[test]
    fn the_fallback_for_kernels_without_openat2_refuses_a_link_as_well() {
        let scratch = Scratch::new("fallback");
        let root = planted(&scratch);
        std::os::unix::fs::symlink(
            scratch.0.join("elsewhere"),
            scratch.users().join(ID).join("servers/away"),
        )
        .unwrap();
        std::fs::create_dir_all(scratch.0.join("elsewhere")).unwrap();
        std::os::unix::fs::symlink("one", scratch.users().join(ID).join("servers/latest")).unwrap();

        let home = root.home(ID).expect("the home");
        for name in ["away", "latest"] {
            let refused = step_in(home.as_fd(), &steps(&["servers", name]), DIRECTORY)
                .expect_err("no link is followed on this road");
            assert!(
                matches!(refused.raw_os_error(), Some(libc::ELOOP) | Some(libc::ENOTDIR)),
                "{name}: {refused}"
            );
        }
        assert!(step_in(home.as_fd(), &steps(&["servers", "one"]), DIRECTORY).is_ok());
    }

    #[test]
    fn a_single_file_is_reached_by_the_same_call_as_a_directory() {
        let scratch = Scratch::new("leaf");
        let home = planted(&scratch).home(ID).expect("the home");

        let dir = home.entry(&steps(&["servers", "one"])).expect("the directory");
        let file = home.entry(&steps(&["servers", "one", "plugins", "a.yml"])).expect("the file");

        assert!(dir.is_dir().expect("a look"));
        assert!(!file.is_dir().expect("a look"));
        assert!(home.dir(&steps(&["servers", "one", "plugins", "a.yml"])).is_err());
    }
}
