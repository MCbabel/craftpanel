use std::ffi::{CString, OsStr};
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};

const FILE_MODE: u32 = 0o660;
const DIR_MODE: u32 = 0o770;

const PANEL_OWNED: &str = "users";

const MAX_BYTES: u64 = 1 << 20;

pub fn read(dir: &Path, name: &str) -> io::Result<Option<Vec<u8>>> {
    let Some(place) = open_dir(dir, false)? else { return Ok(None) };

    let flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    let opened = match openat(place.as_fd(), name.as_bytes(), flags, 0) {
        Ok(fd) => fd,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(explain(err, name)),
    };

    let mut file = File::from(opened);
    let mut bytes = Vec::new();
    Read::take(&mut file, MAX_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(io::Error::other(format!(
            "{name} is larger than {MAX_BYTES} bytes; the panel does not read it"
        )));
    }
    Ok(Some(bytes))
}

pub fn write(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    into_place(dir, name, |file| file.write_all(bytes))
}

pub fn copy(dir: &Path, name: &str, from: &Path) -> io::Result<()> {
    into_place(dir, name, |file| io::copy(&mut File::open(from)?, file).map(|_| ()))
}

pub fn ensure_reachable(dir: &Path) -> io::Result<()> {
    open_dir(dir, true)?;
    Ok(())
}

fn into_place(
    dir: &Path,
    name: &str,
    fill: impl FnOnce(&mut File) -> io::Result<()>,
) -> io::Result<()> {
    let place = open_dir(dir, true)?.ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, format!("{} is not there", dir.display()))
    })?;
    let scratch = format!(".{name}.new");

    remove(place.as_fd(), &scratch)?;

    let flags = libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    let mut file = File::from(
        openat(place.as_fd(), scratch.as_bytes(), flags, FILE_MODE).map_err(|err| explain(err, name))?,
    );

    let written = file
        .set_permissions(std::fs::Permissions::from_mode(FILE_MODE))
        .and_then(|()| fill(&mut file))
        .and_then(|()| file.sync_all());
    drop(file);
    if let Err(err) = written {
        remove(place.as_fd(), &scratch).ok();
        return Err(err);
    }

    renameat(place.as_fd(), &scratch, name).inspect_err(|_| {
        remove(place.as_fd(), &scratch).ok();
    })
}

fn open_dir(dir: &Path, create: bool) -> io::Result<Option<OwnedFd>> {
    let (base, steps) = split_at_base(dir);
    if create {
        std::fs::create_dir_all(&base)?;
    }

    let mut here = match open_directory(&base) {
        Ok(fd) => fd,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };

    let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    for step in steps {
        here = match openat(here.as_fd(), step.as_bytes(), flags, 0) {
            Ok(fd) => fd,
            Err(err) if err.kind() == io::ErrorKind::NotFound && create => {
                mkdirat(here.as_fd(), step.as_bytes())?;
                openat(here.as_fd(), step.as_bytes(), flags, 0)?
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(explain_step(err, &step.to_string_lossy())),
        };
    }
    Ok(Some(here))
}

fn split_at_base(dir: &Path) -> (PathBuf, Vec<&OsStr>) {
    let names: Vec<&OsStr> = dir
        .components()
        .filter_map(|part| match part {
            Component::Normal(name) => Some(name),
            _ => None,
        })
        .collect();

    let Some(at) = names.iter().rposition(|name| *name == OsStr::new(PANEL_OWNED)) else {
        return (dir.to_path_buf(), Vec::new());
    };

    let mut base = dir.to_path_buf();
    for _ in 0..names.len() - at - 1 {
        base.pop();
    }
    (base, names[at + 1..].to_vec())
}

fn explain(err: io::Error, name: &str) -> io::Error {
    if matches!(err.raw_os_error(), Some(libc::ELOOP)) {
        return io::Error::other(format!(
            "{name} is a symbolic link; the panel does not follow one out of a server directory"
        ));
    }
    err
}

fn explain_step(err: io::Error, name: &str) -> io::Error {
    if matches!(err.raw_os_error(), Some(libc::ELOOP | libc::ENOTDIR)) {
        return io::Error::other(format!(
            "{name} is a symbolic link, not a directory; the panel does not follow one on the \
             way into a server directory"
        ));
    }
    err
}

fn open_directory(path: &Path) -> io::Result<OwnedFd> {
    let name = cstring(path.as_os_str().as_bytes())?;
    let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC;
    let raw = unsafe { libc::open(name.as_ptr(), flags) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(raw) })
}

fn openat(dir: BorrowedFd<'_>, name: &[u8], flags: i32, mode: u32) -> io::Result<OwnedFd> {
    let name = cstring(name)?;
    let raw = unsafe { libc::openat(dir.as_raw_fd(), name.as_ptr(), flags, mode as libc::c_uint) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(raw) })
}

fn mkdirat(dir: BorrowedFd<'_>, name: &[u8]) -> io::Result<()> {
    let name = cstring(name)?;
    let made = unsafe { libc::mkdirat(dir.as_raw_fd(), name.as_ptr(), DIR_MODE as libc::mode_t) };
    if made < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn remove(dir: BorrowedFd<'_>, name: &str) -> io::Result<()> {
    let raw = cstring(name.as_bytes())?;
    if unsafe { libc::unlinkat(dir.as_raw_fd(), raw.as_ptr(), 0) } < 0 {
        let err = io::Error::last_os_error();
        if err.kind() != io::ErrorKind::NotFound {
            return Err(err);
        }
    }
    Ok(())
}

fn renameat(dir: BorrowedFd<'_>, from: &str, to: &str) -> io::Result<()> {
    let from = cstring(from.as_bytes())?;
    let to = cstring(to.as_bytes())?;
    let moved = unsafe {
        libc::renameat(dir.as_raw_fd(), from.as_ptr(), dir.as_raw_fd(), to.as_ptr())
    };
    if moved < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn cstring(bytes: &[u8]) -> io::Result<CString> {
    CString::new(bytes).map_err(|_| io::Error::other("a name with a null byte in it"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::harness::a_dir;

    fn a_tree(root: &Path) -> PathBuf {
        let dir = root.join("users").join("01OWNER").join("servers").join("01SERVER");
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_file_goes_out_and_comes_back_and_a_missing_one_is_not_an_error() {
        let dir = a_dir();

        assert_eq!(read(dir.path(), "server.properties").unwrap(), None);
        write(dir.path(), "server.properties", b"motd=x\n").unwrap();
        assert_eq!(read(dir.path(), "server.properties").unwrap().as_deref(), Some(&b"motd=x\n"[..]));

        let left: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, ["server.properties"], "the scratch file never stays behind");
    }

    #[test]
    fn a_link_where_a_file_belongs_is_refused_and_never_read_through() {
        let dir = a_dir();
        let outside = dir.path().join("panel-config.toml");
        std::fs::write(&outside, "secret = true\n").unwrap();
        std::os::unix::fs::symlink(&outside, dir.path().join("server.properties")).unwrap();

        let refusal = read(dir.path(), "server.properties").unwrap_err();
        assert!(
            refusal.to_string().contains("symbolic link"),
            "the panel's own configuration would go out with the page: {refusal}"
        );
    }

    #[test]
    fn a_link_at_the_scratch_name_is_not_written_through_either() {
        let dir = a_dir();
        let outside = dir.path().join("panel-config.toml");
        std::fs::write(&outside, "bind = \"127.0.0.1:8080\"\n").unwrap();
        std::os::unix::fs::symlink(&outside, dir.path().join(".server.properties.new")).unwrap();

        write(dir.path(), "server.properties", b"motd=pwned\n").unwrap();

        assert_eq!(
            std::fs::read_to_string(&outside).unwrap(),
            "bind = \"127.0.0.1:8080\"\n",
            "the file outside the tree has to be untouched"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("server.properties")).unwrap(),
            "motd=pwned\n"
        );
    }

    #[test]
    fn writing_over_a_link_leaves_a_real_file_and_the_target_alone() {
        let dir = a_dir();
        let outside = dir.path().join("elsewhere.txt");
        std::fs::write(&outside, "untouched\n").unwrap();
        std::os::unix::fs::symlink(&outside, dir.path().join("eula.txt")).unwrap();

        write(dir.path(), "eula.txt", b"eula=true\n").unwrap();

        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "untouched\n");
        assert!(!std::fs::symlink_metadata(dir.path().join("eula.txt"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(read(dir.path(), "eula.txt").unwrap().as_deref(), Some(&b"eula=true\n"[..]));
    }

    #[test]
    fn a_copied_jar_lands_whole_and_over_a_link_too() {
        let dir = a_dir();
        let cached = dir.path().join("cached.jar");
        std::fs::write(&cached, b"PK\x03\x04payload").unwrap();
        let outside = dir.path().join("victim.jar");
        std::fs::write(&outside, b"mine").unwrap();
        std::os::unix::fs::symlink(&outside, dir.path().join("server.jar")).unwrap();

        copy(dir.path(), "server.jar", &cached).unwrap();

        assert_eq!(std::fs::read(&outside).unwrap(), b"mine");
        assert_eq!(std::fs::read(dir.path().join("server.jar")).unwrap(), b"PK\x03\x04payload");
    }

    #[test]
    fn the_written_file_is_readable_and_writable_by_the_group() {
        let dir = a_dir();
        write(dir.path(), "server.properties", b"motd=x\n").unwrap();

        let mode = std::fs::metadata(dir.path().join("server.properties")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, FILE_MODE, "the group is the panel (docs/PLAN.md:160-172)");
    }

    #[test]
    fn a_link_anywhere_below_users_is_refused_rather_than_walked_through() {
        for swapped in ["01SERVER", "servers"] {
            let root = a_dir();
            let dir = a_tree(root.path());
            let victim = root.path().join("victim");
            std::fs::create_dir_all(victim.join("servers").join("01SERVER")).unwrap();
            std::fs::write(victim.join("servers").join("01SERVER").join("server.properties"), "motd=theirs\n")
                .unwrap();

            let (target, link) = if swapped == "01SERVER" {
                (victim.join("servers").join("01SERVER"), dir.clone())
            } else {
                (victim.join("servers"), dir.parent().unwrap().to_path_buf())
            };
            std::fs::remove_dir_all(&link).unwrap();
            std::os::unix::fs::symlink(&target, &link).unwrap();

            let refusal = write(&dir, "server.properties", b"motd=mine\n").unwrap_err();
            assert!(
                refusal.to_string().contains("symbolic link"),
                "{swapped} was walked through: {refusal}"
            );
            assert!(read(&dir, "server.properties").is_err(), "{swapped} was read through");
            assert_eq!(
                std::fs::read_to_string(
                    victim.join("servers").join("01SERVER").join("server.properties")
                )
                .unwrap(),
                "motd=theirs\n",
                "another account's tree has to be untouched"
            );
        }
    }

    #[test]
    fn a_missing_server_directory_is_made_on_the_way_and_not_reported_as_a_read() {
        let root = a_dir();
        let dir = root.path().join("users").join("01OWNER").join("servers").join("01SERVER");

        assert_eq!(read(&dir, "server.properties").unwrap(), None);
        assert!(!dir.exists(), "reading makes nothing");

        write(&dir, "eula.txt", b"eula=true\n").unwrap();
        assert_eq!(read(&dir, "eula.txt").unwrap().as_deref(), Some(&b"eula=true\n"[..]));
    }

    #[test]
    fn a_file_too_large_to_be_a_configuration_is_refused_instead_of_swallowed() {
        let dir = a_dir();
        let huge = vec![b'x'; (MAX_BYTES + 1) as usize];
        std::fs::write(dir.path().join("server.properties"), &huge).unwrap();

        let refusal = read(dir.path(), "server.properties").unwrap_err();
        assert!(refusal.to_string().contains("larger than"), "{refusal}");

        std::fs::write(dir.path().join("server.properties"), vec![b'y'; MAX_BYTES as usize])
            .unwrap();
        assert_eq!(read(dir.path(), "server.properties").unwrap().unwrap().len(), MAX_BYTES as usize);
    }

    #[test]
    fn the_walk_starts_at_the_last_users_on_the_path() {
        let (base, steps) =
            split_at_base(Path::new("/srv/users/craftpanel/users/01OWNER/servers/01SERVER"));
        assert_eq!(base, Path::new("/srv/users/craftpanel/users"));
        assert_eq!(steps, ["01OWNER", "servers", "01SERVER"]);

        let (flat, none) = split_at_base(Path::new("/tmp/scratch-7"));
        assert_eq!(flat, Path::new("/tmp/scratch-7"));
        assert!(none.is_empty());
    }
}
