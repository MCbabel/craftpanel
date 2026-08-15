mod beneath;
mod cgroup;
mod users;

use std::io::{BufRead, BufReader, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use craftpanel_proto::{
    is_valid_step, is_valid_user_id, system_username, HelperErrorCode, HelperOk, HelperRequest,
    HelperResponse, SpawnRequest, HELPER_PROTOCOL_VERSION,
};
use tracing_subscriber::EnvFilter;

use beneath::Held;

const DEFAULT_CGROUP_ROOT: &str = "/sys/fs/cgroup/system.slice/craftpanel-games";

struct Settings {
    socket: PathBuf,
    users: beneath::Root,
    supervisor: PathBuf,
    shared_group: String,
    cgroup_root: PathBuf,
}

fn settings() -> Result<Settings> {
    let get = |key: &str, fallback: &str| -> String {
        std::env::var(key).unwrap_or_else(|_| fallback.to_owned())
    };

    let supervisor = PathBuf::from(get("CRAFTPANEL_SUPERVISOR", "/usr/local/bin/craftpanel"));
    if !supervisor.is_absolute() {
        bail!("supervisor path must be absolute");
    }

    let shared_group = get("CRAFTPANEL_GROUP", "craftpanel");
    let users_root = PathBuf::from(get("CRAFTPANEL_USERS_ROOT", "/var/lib/craftpanel/users"));
    let gid = users::group_id(&shared_group)?;
    let (users, repaired) = beneath::Root::prepare(&users_root, gid)
        .with_context(|| format!("opening {}", users_root.display()))?;
    if repaired {
        tracing::warn!(
            path = %users.path().display(),
            "the accounts directory was not root's; it is now, and only root can name an \
             account in it"
        );
    }
    say_if_the_root_can_be_moved(users.path());

    Ok(Settings {
        socket: PathBuf::from(get("CRAFTPANEL_HELPER_SOCKET", "/run/craftpanel/helper.sock")),
        users,
        supervisor,
        shared_group,
        cgroup_root: PathBuf::from(get("CRAFTPANEL_CGROUP_ROOT", DEFAULT_CGROUP_ROOT)),
    })
}

fn say_if_the_root_can_be_moved(users_root: &Path) {
    let Some(parent) = users_root.parent() else { return };
    let Ok(meta) = std::fs::metadata(parent) else { return };
    let mode = meta.permissions().mode();
    let others_may_write = mode & 0o022 != 0 && mode & 0o1000 == 0;

    if meta.uid() != 0 || others_may_write {
        tracing::warn!(
            path = %parent.display(),
            mode = format!("{:o}", mode & 0o7777),
            uid = meta.uid(),
            "this directory is not root's alone, so the accounts directory in it can be \
             renamed aside; run install.sh again to put it right"
        );
    }
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("CRAFTPANEL_LOG").unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let settings = settings()?;

    if let Some(parent) = settings.socket.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let _ = std::fs::remove_file(&settings.socket);

    let listener = UnixListener::bind(&settings.socket)
        .with_context(|| format!("binding {}", settings.socket.display()))?;
    std::fs::set_permissions(&settings.socket, std::fs::Permissions::from_mode(0o660))?;
    users::chown_to_group(&settings.socket, &settings.shared_group)?;

    tracing::info!(socket = %settings.socket.display(), "helper ready");

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                if let Err(err) = serve(&settings, stream) {
                    tracing::warn!("connection ended: {err:#}");
                }
            }
            Err(err) => tracing::warn!("accept failed: {err}"),
        }
    }

    Ok(())
}

fn serve(settings: &Settings, stream: UnixStream) -> Result<()> {
    let reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<HelperRequest>(&line) {
            Ok(request) => dispatch(settings, request),
            Err(err) => Err((HelperErrorCode::MalformedRequest, err.to_string())),
        };

        let response = match response {
            Ok(ok) => HelperResponse::Ok(ok),
            Err((code, message)) => {
                tracing::warn!(?code, "{message}");
                HelperResponse::Error { code, message }
            }
        };

        writeln!(writer, "{}", serde_json::to_string(&response)?)?;
        writer.flush()?;
    }

    Ok(())
}

type Failure = (HelperErrorCode, String);

fn dispatch(settings: &Settings, request: HelperRequest) -> Result<HelperOk, Failure> {
    match request {
        HelperRequest::Ping => Ok(HelperOk::Pong { version: HELPER_PROTOCOL_VERSION }),

        HelperRequest::CreateUser { user_id } => {
            let id = checked_id(&user_id)?;
            users::create(&settings.users, &id, &settings.shared_group)
                .map(|created| HelperOk::UserCreated {
                    uid: created.uid,
                    gid: created.gid,
                    home: created.home,
                })
                .map_err(internal)
        }

        HelperRequest::DeleteUser { user_id, remove_home } => {
            let id = checked_id(&user_id)?;
            users::delete(&id, remove_home).map(|_| HelperOk::UserDeleted).map_err(internal)
        }

        HelperRequest::ApplyLimits { user_id, limits } => {
            let id = checked_id(&user_id)?;
            cgroup::apply(&settings.cgroup_root, &id, &limits)
                .map(|_| HelperOk::LimitsApplied)
                .map_err(|err| (HelperErrorCode::CgroupFailed, format!("{err:#}")))
        }

        HelperRequest::Spawn(request) => spawn(settings, request),

        HelperRequest::ChownTree { user_id, steps } => {
            let id = checked_id(&user_id)?;
            checked_steps(&steps)?;
            let target = reach(settings, &id, &steps, Wanted::Anything)?;
            let account = users::lookup(&system_username(&id))
                .map_err(internal)?
                .ok_or((HelperErrorCode::UnknownUser, format!("no account for {id}")))?;

            users::chown_tree(target, account.uid, &settings.shared_group)
                .map(|entries| HelperOk::TreeChowned { entries })
                .map_err(internal)
        }
    }
}

fn checked_id(user_id: &str) -> Result<String, Failure> {
    if is_valid_user_id(user_id) {
        Ok(user_id.to_owned())
    } else {
        Err((HelperErrorCode::InvalidUserId, format!("not a ULID: {user_id:?}")))
    }
}

fn checked_steps(steps: &[String]) -> Result<(), Failure> {
    match steps.iter().find(|step| !is_valid_step(step)) {
        None => Ok(()),
        Some(bad) => Err((
            HelperErrorCode::PathOutsideRoot,
            format!("{bad:?} is not the name of one thing inside an account"),
        )),
    }
}

enum Wanted {
    Anything,
    Directory,
}

fn reach(
    settings: &Settings,
    user_id: &str,
    steps: &[String],
    wanted: Wanted,
) -> Result<Held, Failure> {
    let home = settings.users.home(user_id).map_err(|err| {
        (
            HelperErrorCode::PathOutsideRoot,
            format!("{} has no usable directory: {err}", system_username(user_id)),
        )
    })?;

    let found = match wanted {
        Wanted::Anything => home.entry(steps),
        Wanted::Directory => home.dir(steps),
    };
    found.map_err(|err| {
        (
            HelperErrorCode::PathOutsideRoot,
            format!("{}/{} cannot be reached: {err}", home.path().display(), steps.join("/")),
        )
    })
}

fn internal(err: anyhow::Error) -> Failure {
    (HelperErrorCode::Internal, format!("{err:#}"))
}

fn spawn(settings: &Settings, request: SpawnRequest) -> Result<HelperOk, Failure> {
    let id = checked_id(&request.user_id)?;
    let account = users::lookup(&system_username(&id))
        .map_err(internal)?
        .ok_or((HelperErrorCode::UnknownUser, format!("no account for {id}")))?;

    if account.uid < 1000 {
        return Err((HelperErrorCode::InvalidUserId, "refusing a system uid".to_owned()));
    }

    let home = settings.users.path().join(&id);
    checked_steps(&request.working_dir)?;
    let working = reach(settings, &id, &request.working_dir, Wanted::Directory)?;
    let working_fd = working.as_fd().as_raw_fd();

    let cgroup = cgroup::ensure(&settings.cgroup_root, &id)
        .map_err(|err| (HelperErrorCode::CgroupFailed, format!("{err:#}")))?;
    let roll = cgroup::open_roll(&cgroup)
        .map_err(|err| (HelperErrorCode::CgroupFailed, format!("{err:#}")))?;
    let roll_fd = roll.as_raw_fd();

    let mut command = Command::new(&settings.supervisor);
    command
        .arg("supervise")
        .arg("--server-id")
        .arg(&request.server_id)
        .arg("--socket")
        .arg(&request.supervisor_socket)
        .arg("--working-dir")
        .arg(working.path())
        .arg("--program")
        .arg(&request.program)
        .arg("--")
        .args(&request.args)
        .env_clear()
        .env("HOME", &home)
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("CRAFTPANEL_SUPERVISOR_TOKEN", &request.token)
        .uid(account.uid)
        .gid(account.gid)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    for (key, value) in &request.env {
        if key.starts_with("CRAFTPANEL_") || key == "LD_PRELOAD" || key == "LD_LIBRARY_PATH" {
            continue;
        }
        command.env(key, value);
    }

    unsafe {
        command.pre_exec(move || {
            libc::umask(0o007);
            if libc::fchdir(working_fd) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            let line = b"0\n";
            if libc::write(roll_fd, line.as_ptr().cast(), line.len()) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let mut child = command
        .spawn()
        .map_err(|err| (HelperErrorCode::SpawnFailed, format!("starting supervisor: {err}")))?;
    let pid = child.id();
    drop(roll);
    drop(working);

    std::thread::spawn(move || {
        let _ = child.wait();
    });

    tracing::info!(pid, server = %request.server_id, uid = account.uid, "supervisor started");
    Ok(HelperOk::Spawned { pid })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;

    const ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const OTHER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FBW";
    const SERVER: &str = "01J1XQZ2K3M4N5P6Q7R8S9TAVB";
    const STRANGER: u32 = 61234;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("craftpanel-helper-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            for id in [ID, OTHER] {
                std::fs::create_dir_all(dir.join("users").join(id).join("servers")).unwrap();
            }
            std::fs::create_dir_all(dir.join("users").join(ID).join("servers").join(SERVER))
                .unwrap();
            Self(dir)
        }

        fn users(&self) -> PathBuf {
            self.0.join("users")
        }

        fn settings(&self) -> Settings {
            Settings {
                socket: self.0.join("helper.sock"),
                users: beneath::Root::open(&self.users()).expect("the accounts directory"),
                supervisor: PathBuf::from("/usr/local/bin/craftpanel"),
                shared_group: "root".to_owned(),
                cgroup_root: self.0.join("cgroup"),
            }
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
        (meta.uid(), meta.gid())
    }

    fn as_root(what: &str) -> bool {
        if unsafe { libc::geteuid() } == 0 {
            return true;
        }
        eprintln!("skipped: {what} only means anything as root");
        false
    }

    #[test]
    fn a_step_that_is_not_the_name_of_one_thing_is_refused() {
        for bad in [vec![".."], vec!["servers", ".."], vec!["/etc"], vec!["servers/one"], vec![""]] {
            let refused = checked_steps(&steps(&bad)).expect_err(&format!("{bad:?} must not pass"));
            assert_eq!(refused.0, HelperErrorCode::PathOutsideRoot, "{bad:?}");
        }
        assert!(checked_steps(&steps(&["servers", SERVER, "plugins"])).is_ok());
    }

    #[test]
    fn a_request_cannot_reach_a_sibling_account() {
        let scratch = Scratch::new("sibling");
        let settings = scratch.settings();

        let refused = reach(&settings, ID, &steps(&["..", OTHER]), Wanted::Anything)
            .expect_err("no way out of the account");
        assert_eq!(refused.0, HelperErrorCode::PathOutsideRoot);
        assert!(reach(&settings, ID, &steps(&["servers"]), Wanted::Anything).is_ok());
    }

    #[test]
    fn a_link_out_of_the_account_is_refused_where_it_stands() {
        let scratch = Scratch::new("escape");
        std::fs::create_dir_all(scratch.0.join("elsewhere")).unwrap();
        std::os::unix::fs::symlink(
            scratch.0.join("elsewhere"),
            scratch.users().join(ID).join("servers/away"),
        )
        .unwrap();

        let refused =
            reach(&scratch.settings(), ID, &steps(&["servers", "away"]), Wanted::Anything)
                .expect_err("a link out of the tree is not a server directory");
        assert_eq!(refused.0, HelperErrorCode::PathOutsideRoot);
    }

    #[test]
    fn a_working_directory_that_is_a_file_is_refused_before_anything_is_started() {
        let scratch = Scratch::new("not-a-dir");
        std::fs::write(scratch.users().join(ID).join("servers/note.txt"), b"x").unwrap();
        let settings = scratch.settings();

        let refused =
            reach(&settings, ID, &steps(&["servers", "note.txt"]), Wanted::Directory)
                .expect_err("a file is not a working directory");
        assert_eq!(refused.0, HelperErrorCode::PathOutsideRoot);
        assert!(reach(&settings, ID, &steps(&["servers", "note.txt"]), Wanted::Anything).is_ok());
    }

    fn confined_by_name(candidate: &Path, home: &Path) -> PathBuf {
        let home = home.canonicalize().expect("a home");
        let resolved = candidate.canonicalize().expect("a target");
        assert!(resolved.starts_with(&home), "the check passes; that was never the weak part");
        resolved
    }

    fn chowned_by_name(resolved: &Path, uid: u32, gid: u32) {
        let name = std::ffi::CString::new(resolved.as_os_str().as_bytes()).expect("a path");
        let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
        let dir = unsafe { libc::open(name.as_ptr(), flags) };
        assert!(dir >= 0, "the second resolution: {}", std::io::Error::last_os_error());
        let held = unsafe { OwnedFd::from_raw_fd(dir) };

        let entry = c"loot";
        unsafe {
            libc::fchmodat(held.as_raw_fd(), entry.as_ptr(), 0o660, libc::AT_SYMLINK_NOFOLLOW);
            libc::fchownat(held.as_raw_fd(), entry.as_ptr(), uid, gid, libc::AT_SYMLINK_NOFOLLOW);
        }
    }

    #[test]
    fn a_middle_segment_swapped_after_the_target_was_decided_reaches_nothing_outside() {
        if !as_root("giving away a file that belongs to root") {
            return;
        }
        let scratch = Scratch::new("probe7");
        let home = scratch.users().join(ID);
        let servers = home.join("servers");
        let inside = servers.join(SERVER).join("plugins");
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::write(inside.join("config.yml"), b"a: 1\n").unwrap();

        let outside = scratch.0.join("outside/plugins");
        std::fs::create_dir_all(&outside).unwrap();
        let loot = outside.join("loot");
        std::fs::write(&loot, b"root's").unwrap();
        std::fs::set_permissions(&loot, std::fs::Permissions::from_mode(0o640)).unwrap();
        std::os::unix::fs::lchown(&loot, Some(0), Some(0)).unwrap();
        let untouched = ((0, 0), 0o640);
        assert_eq!((owner_of(&loot), mode_of(&loot)), untouched);

        let swap = || {
            std::fs::rename(servers.join(SERVER), servers.join("moved-aside")).unwrap();
            std::os::unix::fs::symlink(scratch.0.join("outside"), servers.join(SERVER)).unwrap();
        };
        let put_back = || {
            std::fs::remove_file(servers.join(SERVER)).unwrap();
            std::fs::rename(servers.join("moved-aside"), servers.join(SERVER)).unwrap();
        };

        let blessed = confined_by_name(&inside, &home);
        swap();
        chowned_by_name(&blessed, STRANGER, 0);
        put_back();
        assert_eq!(
            (owner_of(&loot), mode_of(&loot)),
            ((STRANGER, 0), 0o660),
            "the probe has to reproduce, or the half below it measures nothing"
        );

        std::fs::set_permissions(&loot, std::fs::Permissions::from_mode(0o640)).unwrap();
        std::os::unix::fs::lchown(&loot, Some(0), Some(0)).unwrap();

        let settings = scratch.settings();
        let target = reach(&settings, ID, &steps(&["servers", SERVER, "plugins"]), Wanted::Anything)
            .expect("the plugins directory");
        swap();
        let touched = users::chown_tree(target, STRANGER, &settings.shared_group);
        put_back();

        assert_eq!(touched.expect("the tree is handed back"), 2, "the directory and its file");
        assert_eq!(
            (owner_of(&loot), mode_of(&loot)),
            untouched,
            "the file outside the tree kept its owner and its mode"
        );
        assert_eq!(mode_of(&inside), 0o2770, "and the tree that was asked for did get its turn");
        assert_eq!(owner_of(&inside).0, STRANGER);
    }

    #[test]
    fn a_middle_segment_that_is_a_link_is_refused_rather_than_walked_through() {
        let scratch = Scratch::new("middle-link");
        let servers = scratch.users().join(ID).join("servers");
        std::fs::create_dir_all(scratch.0.join("outside/plugins")).unwrap();
        std::fs::remove_dir(servers.join(SERVER)).unwrap();
        std::os::unix::fs::symlink(scratch.0.join("outside"), servers.join(SERVER)).unwrap();

        let refused = reach(
            &scratch.settings(),
            ID,
            &steps(&["servers", SERVER, "plugins"]),
            Wanted::Anything,
        )
        .expect_err("nothing behind that link may be reached");
        assert_eq!(refused.0, HelperErrorCode::PathOutsideRoot);
    }
}
