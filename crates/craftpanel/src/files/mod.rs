#![allow(dead_code)]

pub mod archive;
pub mod jail;
pub mod path;

use std::io;
use std::path::PathBuf;

use axum::http::StatusCode;
use serde::{Deserialize, Serialize};

use crate::auth::error::{Failure, Result};
use crate::config::Config;
use crate::helper::Helper;
use crate::model::Id;

pub use jail::{Dir, Kind, Meta, Part, Root};
pub use path::{PathFault, RelPath};

pub const MAX_TEXT_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_PAGE_SIZE: u32 = 5_000;
pub const DEFAULT_PAGE_SIZE: u32 = 1_000;
pub const MAX_EXTRACT_UNCOMPRESSED_BYTES: u64 = 20 * 1024 * 1024 * 1024;
pub const MAX_EXTRACT_ENTRIES: u64 = 200_000;
pub const MAX_CONFLICTS: usize = 200;

pub const WORK_DIR: &str = crate::ops::WORK_DIR;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Measured {
    pub bytes: u64,
    pub unreadable: u32,
}

impl Measured {
    pub fn complete(self) -> bool {
        self.unreadable == 0
    }
}

pub fn tree_size(dir: &std::path::Path) -> u64 {
    measure(dir).bytes
}

pub fn measure(dir: &std::path::Path) -> Measured {
    let mut found = Measured::default();
    for step in walkdir::WalkDir::new(dir).follow_links(false) {
        match step.and_then(|entry| entry.metadata()) {
            Ok(meta) if meta.is_file() => found.bytes = found.bytes.saturating_add(meta.len()),
            Ok(_) => {}
            Err(err) if err.io_error().is_some_and(|io| io.kind() == io::ErrorKind::NotFound) => {}
            Err(_) => found.unreadable += 1,
        }
    }
    found
}

pub fn filesystem_total_bytes(path: &std::path::Path) -> u64 {
    use std::os::unix::ffi::OsStrExt;

    let Ok(name) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return 0;
    };
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(name.as_ptr(), &mut stats) } != 0 {
        return 0;
    }
    (stats.f_blocks as u64).saturating_mul(stats.f_frsize as u64)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ApiFileItem {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub path: String,
    pub modified: i64,
    pub created: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

impl ApiFileItem {
    pub fn new(path: &RelPath, name: String, meta: Meta) -> Self {
        Self {
            name,
            kind: meta.kind.on_the_wire(),
            path: path.on_the_wire(),
            modified: meta.modified,
            created: meta.created,
            size: matches!(meta.kind, Kind::File | Kind::Other).then_some(meta.size),
            count: None,
            target: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesMetaResponse {
    pub root_path: String,
    pub max_upload_bytes: u64,
    pub max_text_bytes: u64,
    pub max_page_size: u32,
    pub default_page_size: u32,
    pub max_extract_uncompressed_bytes: u64,
    pub max_extract_entries: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ListDirectoryResponse {
    pub path: String,
    pub page_size: u32,
    pub total: u64,
    pub has_more: bool,
    pub next_after: Option<String>,
    pub items: Vec<ApiFileItem>,
}

pub struct Workspace {
    root: Root,
    owner: Id,
    server: Id,
    helper: Helper,
}

impl Workspace {
    pub fn open(config: &Config, owner: Id, server: Id) -> Result<Self> {
        let root = Root::open_beneath(config.users_dir(), &steps_to(owner, server)).map_err(
            |err| match err.raw_os_error() {
                Some(libc::ELOOP) | Some(libc::EXDEV) => {
                    tracing::error!(
                        "the directory of server {server} is a link, and a link is not a \
                         server directory: {err}"
                    );
                    Failure::new(
                        StatusCode::FORBIDDEN,
                        "forbidden_path",
                        "the directory of this server is not where it belongs",
                    )
                }
                Some(libc::ENOENT) | Some(libc::ENOTDIR) => {
                    Failure::not_found("not_found", "this server has no directory yet")
                }
                _ => Failure::internal(
                    anyhow::Error::new(err).context("opening the server directory"),
                ),
            },
        )?;
        Ok(Self { root, owner, server, helper: Helper::new(&config.helper_socket) })
    }

    pub fn root(&self) -> &Root {
        &self.root
    }

    pub async fn hand_back(&self, rel: &RelPath) -> Result<()> {
        if matches!(self.root.meta(rel), Ok(meta) if meta.kind == Kind::Symlink) {
            return Ok(());
        }
        let steps = crate::helper::below_server(self.server, rel.segments());
        self.helper.chown_tree(&self.owner.to_string(), steps).await.map_err(|err| {
            Failure::internal(err.context(format!(
                "{} still belongs to the panel and the game process cannot read it",
                self.root.full_path(rel).display()
            )))
        })?;
        Ok(())
    }
}

pub fn server_dir(config: &Config, owner: Id, server: Id) -> PathBuf {
    let mut path = config.users_dir();
    for step in steps_to(owner, server) {
        path.push(step);
    }
    path
}

fn steps_to(owner: Id, server: Id) -> [String; 3] {
    [owner.to_string(), "servers".to_owned(), server.to_string()]
}

pub fn fault(err: &io::Error, missing: &'static str) -> Failure {
    let code = err.raw_os_error().unwrap_or(0);
    match code {
        libc::EXDEV | libc::ELOOP => Failure::new(
            StatusCode::FORBIDDEN,
            "forbidden_path",
            "this path leaves the server directory",
        ),
        libc::EACCES | libc::EPERM => {
            Failure::conflict("file_not_accessible", "the game left this closed to the panel")
        }
        libc::ENOENT | libc::ENOTDIR => Failure::not_found(missing, "no such path"),
        libc::EEXIST => Failure::conflict("already_exists", "something is already there"),
        libc::ENOTEMPTY => Failure::conflict("not_empty", "this directory is not empty"),
        libc::ENOSPC | libc::EDQUOT | libc::EFBIG => {
            Failure::new(StatusCode::INSUFFICIENT_STORAGE, "no_space", "the disk is full")
        }
        libc::ENAMETOOLONG => Failure::bad_request("path_too_long", "this path is too long"),
        libc::EISDIR => Failure::bad_request("not_a_regular_file", "this is a directory"),
        _ => Failure::internal(anyhow::Error::new(io::Error::from_raw_os_error(code))),
    }
}

impl From<PathFault> for Failure {
    fn from(fault: PathFault) -> Self {
        Self::bad_request(fault.code(), fault.message())
    }
}

pub fn page(
    root: &Root,
    at: &RelPath,
    after: Option<&str>,
    page_size: u32,
) -> Result<ListDirectoryResponse> {
    let page_size = page_size.max(1);
    let dir = root.dir(at).map_err(|err| match err.raw_os_error() {
        Some(libc::ENOTDIR) => Failure::bad_request("not_a_directory", "this is not a directory"),
        _ => fault(&err, "not_found"),
    })?;

    let mut raw_names = dir.entries().map_err(|err| fault(&err, "not_found"))?;
    if at.is_root() {
        raw_names.retain(|name| name != WORK_DIR.as_bytes());
    }

    let mut names: Vec<(String, Vec<u8>)> = raw_names
        .into_iter()
        .map(|raw| (String::from_utf8_lossy(&raw).into_owned(), raw))
        .collect();
    names.sort_unstable();

    let total = names.len() as u64;
    let start =
        after.map_or(0, |after| names.partition_point(|(shown, _)| shown.as_str() <= after));
    let mut taken: Vec<(String, Vec<u8>)> = names.into_iter().skip(start).collect();

    let mut cut = (page_size as usize).min(taken.len());
    while cut > 0 && cut < taken.len() && taken[cut].0 == taken[cut - 1].0 {
        cut += 1;
    }
    let has_more = taken.len() > cut;
    taken.truncate(cut);
    let next_after =
        has_more.then(|| taken.last().map(|(shown, _)| shown.clone())).flatten();

    let mut items = Vec::with_capacity(taken.len());
    for (name, raw) in taken {
        let Ok(meta) = dir.meta(&raw) else {
            continue;
        };

        let mut item = ApiFileItem::new(&at.with_name(&name), name, meta);
        match meta.kind {
            Kind::Directory => item.count = dir.child(&raw).and_then(|sub| sub.count()).ok(),
            Kind::Symlink => item.target = dir.read_link(&raw).ok(),
            _ => {}
        }
        items.push(item);
    }

    Ok(ListDirectoryResponse {
        path: at.on_the_wire(),
        page_size,
        total,
        has_more,
        next_after,
        items,
    })
}

pub fn non_utf8_name() -> Failure {
    Failure::bad_request("non_utf8_name", "this entry has a name that is not UTF-8")
}

pub fn looks_lossy(name: &str) -> bool {
    name.contains('\u{fffd}')
}

pub const PART_LIFETIME: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

pub async fn sweep_parts(pool: &sqlx::SqlitePool, config: &Config) -> Result<usize> {
    let servers: Vec<(Id, Id)> =
        sqlx::query_as("SELECT id, owner_id FROM servers").fetch_all(pool).await?;
    let cutoff = std::time::SystemTime::now() - PART_LIFETIME;
    let users = config.users_dir();
    let roots: Vec<[String; 3]> =
        servers.into_iter().map(|(server, owner)| steps_to(owner, server)).collect();

    let swept = tokio::task::spawn_blocking(move || {
        roots
            .into_iter()
            .filter_map(|steps| Root::open_beneath(&users, &steps).ok())
            .map(|root| sweep_below(&root, &RelPath::root(), cutoff, 0))
            .sum()
    })
    .await
    .map_err(|err| Failure::internal(anyhow::Error::new(err)))?;

    Ok(swept)
}

fn sweep_below(root: &Root, at: &RelPath, cutoff: std::time::SystemTime, depth: usize) -> usize {
    if depth > path::MAX_DEPTH {
        return 0;
    }
    let Ok(dir) = root.dir(at) else {
        return 0;
    };
    let Ok(names) = dir.entries() else {
        return 0;
    };

    let mut swept = 0;
    for raw in names {
        let name = String::from_utf8_lossy(&raw).into_owned();
        let Ok(meta) = dir.meta(&raw) else {
            continue;
        };
        match meta.kind {
            Kind::Directory => swept += sweep_below(root, &at.with_name(&name), cutoff, depth + 1),
            Kind::File if is_part(&name) && old_enough(meta.modified, cutoff) => {
                if dir.unlink(&raw).is_ok() {
                    swept += 1;
                }
            }
            _ => {}
        }
    }
    swept
}

fn is_part(name: &str) -> bool {
    name.starts_with('.')
        && name.rsplit_once(".part.").is_some_and(|(stem, tail)| {
            !stem.is_empty() && tail.parse::<Id>().is_ok()
        })
}

fn old_enough(modified: i64, cutoff: std::time::SystemTime) -> bool {
    let seconds = cutoff
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64);
    modified < seconds
}

#[cfg(test)]
pub mod testing {
    use std::path::{Path, PathBuf};

    use crate::config::Config;
    use crate::model::Id;

    pub struct Sandbox {
        data: PathBuf,
        pub owner: Id,
        pub server: Id,
    }

    impl Sandbox {
        pub fn new() -> Self {
            let data = std::env::temp_dir()
                .join(format!("craftpanel-files-{}-{}", std::process::id(), Id::new()));
            let sandbox = Self { data, owner: Id::new(), server: Id::new() };
            std::fs::create_dir_all(sandbox.server_dir()).expect("a server directory");
            sandbox
        }

        pub fn config(&self) -> Config {
            Config { data_dir: self.data.clone(), ..Config::default() }
        }

        pub fn data_dir(&self) -> &Path {
            &self.data
        }

        pub fn server_dir(&self) -> PathBuf {
            super::server_dir(&self.config(), self.owner, self.server)
        }

        pub fn write(&self, rel: &str, bytes: &[u8]) -> PathBuf {
            let path = self.server_dir().join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("a parent directory");
            }
            std::fs::write(&path, bytes).expect("a file");
            path
        }

        pub fn mkdir(&self, rel: &str) -> PathBuf {
            let path = self.server_dir().join(rel);
            std::fs::create_dir_all(&path).expect("a directory");
            path
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.data).ok();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::Sandbox;
    use super::*;
    use std::os::unix::ffi::OsStringExt;
    use std::time::Duration;

    fn at(raw: &str) -> RelPath {
        RelPath::parse(raw).expect("a usable path")
    }

    fn root(sandbox: &Sandbox) -> Root {
        Root::open(sandbox.server_dir()).expect("the root opens")
    }

    struct NotRoot;

    impl NotRoot {
        fn take() -> Option<Self> {
            const NOBODY: libc::uid_t = 65534;
            if unsafe { libc::geteuid() } != 0 {
                return None;
            }
            unsafe { libc::setfsuid(NOBODY) };
            let now = unsafe { libc::setfsuid(libc::uid_t::MAX) };
            assert_eq!(now, NOBODY as libc::c_int, "this test needs to be able to stop being root");
            Some(Self)
        }
    }

    impl Drop for NotRoot {
        fn drop(&mut self) {
            unsafe { libc::setfsuid(0) };
        }
    }

    #[test]
    fn a_directory_the_panel_is_refused_is_counted_as_unread_not_as_nothing() {
        use std::os::unix::fs::PermissionsExt;

        let sandbox = Sandbox::new();
        sandbox.write("world/level.dat", &[b'x'; 100]);
        let shut = sandbox.mkdir("plugins/WorldEdit/lang");
        std::fs::write(shut.join("strings.json"), [b'y'; 50]).expect("a file the game wrote");
        let closed = sandbox.server_dir().join("plugins/WorldEdit");
        std::fs::set_permissions(&closed, PermissionsExt::from_mode(0o000)).expect("a shut door");

        let stranger = NotRoot::take();
        let found = measure(&sandbox.server_dir());
        drop(stranger);
        std::fs::set_permissions(&closed, PermissionsExt::from_mode(0o755)).expect("open again");

        assert_eq!(found.bytes, 100, "what could be read is counted");
        assert_eq!(found.unreadable, 1, "and what could not is not passed over in silence");
        assert!(!found.complete());
        assert_eq!(tree_size(&sandbox.server_dir()), 150, "with the door open, all of it");
    }

    #[test]
    fn a_tree_that_reads_all_the_way_through_is_complete() {
        let sandbox = Sandbox::new();
        sandbox.write("world/level.dat", &[b'x'; 100]);
        std::os::unix::fs::symlink("world/level.dat", sandbox.server_dir().join("link"))
            .expect("a link");

        let found = measure(&sandbox.server_dir());
        assert_eq!(found, Measured { bytes: 100, unreadable: 0 }, "a link is not a file of its own");
        assert!(found.complete());

        let nowhere = measure(&sandbox.server_dir().join("not-installed-yet"));
        assert_eq!(nowhere, Measured::default());
        assert!(nowhere.complete());
    }

    #[test]
    fn a_page_carries_the_shape_the_interface_assigns_straight_to_fileitem() {
        let sandbox = Sandbox::new();
        sandbox.write("server.properties", b"level-name=world\n");
        sandbox.mkdir("plugins/config");
        sandbox.write("plugins/config/one.yml", b"a: 1\n");
        std::os::unix::fs::symlink("config/one.yml", sandbox.server_dir().join("plugins/link.yml"))
            .expect("a link");

        let listed = page(&root(&sandbox), &at("/plugins"), None, 10).expect("a page");
        assert_eq!(listed.path, "/plugins");
        assert_eq!(listed.total, 2);
        assert!(!listed.has_more);
        assert_eq!(listed.next_after, None);

        let names: Vec<&str> = listed.items.iter().map(|item| item.name.as_str()).collect();
        assert_eq!(names, ["config", "link.yml"], "the order is the byte order of the name");

        let config = &listed.items[0];
        assert_eq!(config.kind, "directory");
        assert_eq!(config.path, "/plugins/config");
        assert_eq!(config.count, Some(1));
        assert_eq!(config.size, None);
        assert!(config.modified > 1_700_000_000, "unix seconds, not a string");

        let link = &listed.items[1];
        assert_eq!(link.kind, "symlink", "lstat, not stat: deleting it must not follow");
        assert_eq!(link.target.as_deref(), Some("config/one.yml"));
    }

    #[test]
    fn paging_walks_every_entry_exactly_once() {
        let sandbox = Sandbox::new();
        for index in 0..25 {
            sandbox.write(&format!("file-{index:02}.txt"), b"x");
        }

        let mut seen = Vec::new();
        let mut after: Option<String> = None;
        loop {
            let listed = page(&root(&sandbox), &RelPath::root(), after.as_deref(), 7).unwrap();
            assert_eq!(listed.total, 25);
            seen.extend(listed.items.iter().map(|item| item.name.clone()));
            match listed.next_after {
                Some(next) if listed.has_more => after = Some(next),
                _ => break,
            }
        }

        assert_eq!(seen.len(), 25, "no entry twice and none missing");
        let mut unique = seen.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), 25);
        assert_eq!(seen[0], "file-00.txt");
    }

    #[test]
    fn the_work_directory_of_a_run_is_not_listed() {
        let sandbox = Sandbox::new();
        sandbox.mkdir(WORK_DIR);
        sandbox.write("mods/a.jar", b"x");

        let listed = page(&root(&sandbox), &RelPath::root(), None, 100).expect("a page");
        let names: Vec<&str> = listed.items.iter().map(|item| item.name.as_str()).collect();
        assert_eq!(names, ["mods"]);
        assert_eq!(listed.total, 1);

        sandbox.mkdir(&format!("mods/{WORK_DIR}"));
        let inside = page(&root(&sandbox), &at("/mods"), None, 100).expect("a page");
        assert_eq!(inside.items.len(), 2);
    }

    #[test]
    fn a_part_file_is_listed_because_it_takes_up_space() {
        let sandbox = Sandbox::new();
        sandbox.write(".world.zip.part.01ARZ3NDEKTSV4RRFFQ69G5FAV", b"half of it");

        let listed = page(&root(&sandbox), &RelPath::root(), None, 100).expect("a page");
        assert_eq!(listed.items.len(), 1, "7.8: hiding it would be a lie about the disk");
    }

    #[test]
    fn listing_a_file_is_not_a_directory_and_a_missing_one_is_not_found() {
        let sandbox = Sandbox::new();
        sandbox.write("server.properties", b"x");

        let refused = page(&root(&sandbox), &at("/server.properties"), None, 10).unwrap_err();
        assert_eq!(refused.code(), "not_a_directory");
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST);

        let missing = page(&root(&sandbox), &at("/nowhere"), None, 10).unwrap_err();
        assert_eq!(missing.code(), "not_found");
    }

    #[test]
    fn a_name_that_is_not_utf8_does_not_wedge_the_cursor() {
        let sandbox = Sandbox::new();
        sandbox.write("a.txt", b"a");
        std::fs::write(
            sandbox.server_dir().join(std::ffi::OsString::from_vec(vec![b'm', 0xff, b'd'])),
            b"x",
        )
        .expect("a file with a broken name");
        sandbox.write("z.txt", b"z");

        let mut seen = Vec::new();
        let mut after: Option<String> = None;
        for _ in 0..10 {
            let listed = page(&root(&sandbox), &RelPath::root(), after.as_deref(), 1).unwrap();
            seen.extend(listed.items.iter().map(|item| item.name.clone()));
            match listed.next_after {
                Some(next) if listed.has_more => after = Some(next),
                _ => break,
            }
        }

        assert_eq!(seen.len(), 3, "three entries, each exactly once: {seen:?}");
        assert!(seen.contains(&"a.txt".to_owned()));
        assert!(seen.contains(&"z.txt".to_owned()), "the entry behind the broken name is reachable");
    }

    #[test]
    fn two_entries_that_show_the_same_name_are_both_listed() {
        let sandbox = Sandbox::new();
        for byte in [0xfe, 0xff] {
            std::fs::write(
                sandbox.server_dir().join(std::ffi::OsString::from_vec(vec![b'm', byte, b'd'])),
                b"x",
            )
            .expect("a file with a broken name");
        }
        sandbox.write("z.txt", b"z");

        let mut seen = Vec::new();
        let mut after: Option<String> = None;
        for _ in 0..10 {
            let listed = page(&root(&sandbox), &RelPath::root(), after.as_deref(), 1).unwrap();
            assert_eq!(listed.total, 3);
            seen.extend(listed.items.iter().map(|item| item.name.clone()));
            match listed.next_after {
                Some(next) if listed.has_more => after = Some(next),
                _ => break,
            }
        }

        assert_eq!(seen.len(), 3, "every entry leaves exactly once: {seen:?}");
        assert!(seen.contains(&"z.txt".to_owned()), "and the one behind them is reachable");
    }

    #[test]
    fn a_name_that_is_not_utf8_is_still_listed() {
        let sandbox = Sandbox::new();
        let broken = std::ffi::OsString::from_vec(vec![b'm', 0xff, b'd', b'.', b'j']);
        std::fs::write(sandbox.server_dir().join(&broken), b"x").expect("a file");

        let listed = page(&root(&sandbox), &RelPath::root(), None, 10).expect("a page");
        assert_eq!(listed.items.len(), 1, "the user has to be able to see it");
        assert!(listed.items[0].name.contains('\u{fffd}'));
    }

    #[test]
    fn only_a_part_file_that_nobody_is_writing_any_more_is_swept() {
        let sandbox = Sandbox::new();
        let stale = sandbox.write(".world.zip.part.01ARZ3NDEKTSV4RRFFQ69G5FAV", b"half");
        let fresh = sandbox.write("config/.one.yml.part.01ARZ3NDEKTSV4RRFFQ69G5FAW", b"half");
        let ordinary = sandbox.write("notes.part.txt", b"mine");
        let disguised = sandbox.write(".a.part.not-a-ulid", b"mine too");

        let long_ago = std::time::SystemTime::now() - PART_LIFETIME - Duration::from_secs(3600);
        std::fs::File::open(&stale)
            .and_then(|file| file.set_times(std::fs::FileTimes::new().set_modified(long_ago)))
            .expect("an old part file");

        let root = Root::open(sandbox.server_dir()).expect("the root");
        let cutoff = std::time::SystemTime::now() - PART_LIFETIME;
        assert_eq!(sweep_below(&root, &RelPath::root(), cutoff, 0), 1);

        assert!(!stale.exists(), "a write nobody finished goes");
        assert!(fresh.exists(), "one that is minutes old is an upload in flight");
        assert!(ordinary.exists());
        assert!(disguised.exists(), "the tail has to be a ULID, or it is somebody's file");
    }

    #[test]
    fn errno_becomes_the_code_the_interface_switches_on() {
        let cases = [
            (libc::EXDEV, "forbidden_path", StatusCode::FORBIDDEN),
            (libc::ELOOP, "forbidden_path", StatusCode::FORBIDDEN),
            (libc::ENOENT, "not_found", StatusCode::NOT_FOUND),
            (libc::EEXIST, "already_exists", StatusCode::CONFLICT),
            (libc::ENOTEMPTY, "not_empty", StatusCode::CONFLICT),
            (libc::ENOSPC, "no_space", StatusCode::INSUFFICIENT_STORAGE),
            (libc::EDQUOT, "no_space", StatusCode::INSUFFICIENT_STORAGE),
            (libc::EACCES, "file_not_accessible", StatusCode::CONFLICT),
            (libc::EPERM, "file_not_accessible", StatusCode::CONFLICT),
            (libc::EIO, "internal", StatusCode::INTERNAL_SERVER_ERROR),
        ];
        for (errno, code, status) in cases {
            let failure = fault(&io::Error::from_raw_os_error(errno), "not_found");
            assert_eq!(failure.code(), code, "errno {errno}");
            assert_eq!(failure.status(), status, "errno {errno}");
        }
        assert_eq!(
            fault(&io::Error::from_raw_os_error(libc::ENOENT), "parent_not_found").code(),
            "parent_not_found"
        );
    }

    fn mode_of(path: &std::path::Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::symlink_metadata(path).expect("the entry").permissions().mode() & 0o7777
    }

    #[tokio::test]
    async fn a_hand_back_lands_on_the_tree_the_steps_name() {
        use std::os::unix::fs::PermissionsExt;

        let sandbox = Sandbox::new();
        let helper = crate::auth::harness::FakeHelper::obliging()
            .await
            .rooted_at(sandbox.data_dir().join("users"));
        let mut config = sandbox.config();
        config.helper_socket = helper.socket();

        let shut = sandbox.mkdir("plugins/WorldEdit");
        let inside = sandbox.write("plugins/WorldEdit/config.yml", b"a: 1\n");
        let untouched = sandbox.write("world/level.dat", b"x");
        for path in [&shut, &inside, &untouched] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }

        let workspace = Workspace::open(&config, sandbox.owner, sandbox.server).expect("a tree");
        workspace.hand_back(&at("/plugins")).await.expect("the hand-back");

        assert_eq!(mode_of(&shut), 0o2770, "the directory the steps named");
        assert_eq!(mode_of(&inside), 0o660, "and everything under it");
        assert_eq!(mode_of(&untouched), 0o600, "and nothing beside it");
    }
}
