#![cfg(test)]

use std::collections::HashMap;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Path as RoutePath, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Json;
use axum::Router;

use crate::loaders::checksum::{self, Algorithm};

#[derive(Clone)]
struct Build {
    version: String,
    bytes: Vec<u8>,
    announced: Option<String>,
    announced_size: Option<u64>,
}

struct Fake {
    base: String,
    builds: Mutex<HashMap<u32, Build>>,
    queries: Mutex<Vec<String>>,
    asked: AtomicUsize,
    served: AtomicUsize,
    hold: Mutex<Option<Duration>>,
    gate: tokio::sync::watch::Sender<bool>,
    link: Mutex<Option<String>>,
    detour: Mutex<Option<String>>,
}

pub struct FakeAdoptium {
    base: String,
    state: Arc<Fake>,
}

impl FakeAdoptium {
    pub async fn started() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("a free port");
        let base = format!("http://{}", listener.local_addr().expect("an address"));

        let state = Arc::new(Fake {
            base: base.clone(),
            builds: Mutex::default(),
            queries: Mutex::default(),
            asked: AtomicUsize::new(0),
            served: AtomicUsize::new(0),
            hold: Mutex::default(),
            gate: tokio::sync::watch::channel(true).0,
            link: Mutex::default(),
            detour: Mutex::default(),
        });
        let app = Router::new()
            .route("/v3/assets/latest/{major}/hotspot", get(latest))
            .route("/binaries/{name}", get(binary))
            .route("/detour/{name}", get(detour))
            .with_state(Arc::clone(&state));

        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        Self { base, state }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn offer(&self, major: u32, version: &str, bytes: Vec<u8>) {
        let announced = Some(checksum::digest(Algorithm::Sha256, &bytes));
        self.put(major, version, bytes, announced);
    }

    pub fn offer_announcing(&self, major: u32, version: &str, bytes: Vec<u8>, announced: &str) {
        self.put(major, version, bytes, Some(announced.to_owned()));
    }

    pub fn offer_unchecked(&self, major: u32, version: &str, bytes: Vec<u8>) {
        self.put(major, version, bytes, None);
    }

    pub fn offer_sized(&self, major: u32, version: &str, bytes: Vec<u8>, announced: u64) {
        self.offer(major, version, bytes);
        if let Some(build) = self.state.builds.lock().expect("the builds").get_mut(&major) {
            build.announced_size = Some(announced);
        }
    }

    pub fn binary_url(&self, version: &str) -> String {
        format!("{}/binaries/{version}.tar.gz", self.base)
    }

    pub fn detour_url(&self, version: &str) -> String {
        format!("{}/detour/{version}.tar.gz", self.base)
    }

    pub fn point_at(&self, url: &str) {
        *self.state.link.lock().expect("the link") = Some(url.to_owned());
    }

    pub fn detour_to(&self, url: &str) {
        *self.state.detour.lock().expect("the detour") = Some(url.to_owned());
    }

    pub fn hold(&self, how_long: Duration) {
        *self.state.hold.lock().expect("the hold") = Some(how_long);
    }

    pub fn shut(&self) {
        self.state.gate.send_replace(false);
    }

    pub fn open(&self) {
        self.state.gate.send_replace(true);
    }

    pub fn asked(&self) -> usize {
        self.state.asked.load(Ordering::Relaxed)
    }

    pub fn served(&self) -> usize {
        self.state.served.load(Ordering::Relaxed)
    }

    pub fn queries(&self) -> Vec<String> {
        self.state.queries.lock().expect("the queries").clone()
    }

    fn put(&self, major: u32, version: &str, bytes: Vec<u8>, announced: Option<String>) {
        self.state
            .builds
            .lock()
            .expect("the builds")
            .insert(
                major,
                Build { version: version.to_owned(), bytes, announced, announced_size: None },
            );
    }
}

async fn latest(
    State(state): State<Arc<Fake>>,
    RoutePath(major): RoutePath<u32>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    state.asked.fetch_add(1, Ordering::Relaxed);
    let mut named: Vec<String> =
        query.iter().map(|(key, value)| format!("{key}={value}")).collect();
    named.sort();
    state.queries.lock().expect("the queries").push(named.join("&"));

    let Some(build) = state.builds.lock().expect("the builds").get(&major).cloned() else {
        return Json(serde_json::json!([])).into_response();
    };

    let name = format!("OpenJDK{major}U-jre_x64_linux_hotspot_{}.tar.gz", build.version);
    let link = state
        .link
        .lock()
        .expect("the link")
        .clone()
        .unwrap_or_else(|| format!("{}/binaries/{name}", state.base));
    let mut package = serde_json::json!({
        "link": link,
        "name": name,
        "size": build.announced_size.unwrap_or(build.bytes.len() as u64),
    });
    if let Some(announced) = &build.announced {
        package["checksum"] = serde_json::Value::String(announced.clone());
    }

    Json(serde_json::json!([{
        "binary": { "architecture": "x64", "image_type": "jre", "os": "linux", "package": package },
        "release_name": format!("jdk-{}", build.version),
        "vendor": "eclipse",
        "version": { "major": major, "semver": build.version },
    }]))
    .into_response()
}

async fn binary(State(state): State<Arc<Fake>>, RoutePath(name): RoutePath<String>) -> Response {
    state.served.fetch_add(1, Ordering::Relaxed);
    let mut gate = state.gate.subscribe();
    while !*gate.borrow_and_update() {
        if gate.changed().await.is_err() {
            break;
        }
    }
    let held = *state.hold.lock().expect("the hold");
    if let Some(how_long) = held {
        tokio::time::sleep(how_long).await;
    }

    let found = state
        .builds
        .lock()
        .expect("the builds")
        .values()
        .find(|build| name.contains(&build.version))
        .map(|build| build.bytes.clone());

    match found {
        Some(bytes) => bytes.into_response(),
        None => (StatusCode::NOT_FOUND, "no such binary").into_response(),
    }
}

async fn detour(State(state): State<Arc<Fake>>) -> Response {
    match state.detour.lock().expect("the detour").clone() {
        Some(target) => {
            (StatusCode::FOUND, [(axum::http::header::LOCATION, target)]).into_response()
        }
        None => (StatusCode::NOT_FOUND, "nowhere to go").into_response(),
    }
}

pub fn a_jre(version: &str) -> Vec<u8> {
    let root = format!("jdk-{version}-jre");
    let release = format!(
        "IMPLEMENTOR=\"Eclipse Adoptium\"\nJAVA_VERSION=\"{version}\"\nOS_ARCH=\"x86_64\"\n"
    );
    let launcher = format!("#!/bin/sh\necho 'openjdk version \"{version}\"' 1>&2\n");

    tarball(|builder| {
        directory(builder, &format!("{root}/"), 0o755);
        directory(builder, &format!("{root}/bin"), 0o755);
        file(builder, &format!("{root}/bin/java"), launcher.as_bytes(), 0o755);
        file(builder, &format!("{root}/release"), release.as_bytes(), 0o644);
        file(builder, &format!("{root}/lib/libjsig.so"), b"not really a library", 0o644);
        link(builder, &format!("{root}/lib/server/libjsig.so"), "../libjsig.so");
    })
}

pub fn tarball(fill: impl FnOnce(&mut tar::Builder<flate2::write::GzEncoder<Vec<u8>>>)) -> Vec<u8> {
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut builder = tar::Builder::new(encoder);
    fill(&mut builder);
    let mut done = builder.into_inner().expect("the encoder back");
    done.flush().expect("a flushed archive");
    done.finish().expect("a finished archive")
}

pub fn file<W: Write>(builder: &mut tar::Builder<W>, name: &str, body: &[u8], mode: u32) {
    let mut header = tar::Header::new_gnu();
    header.set_size(body.len() as u64);
    header.set_mode(mode);
    builder.append_data(&mut header, name, body).expect("an entry");
}

pub fn directory<W: Write>(builder: &mut tar::Builder<W>, name: &str, mode: u32) {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Directory);
    header.set_size(0);
    header.set_mode(mode);
    builder.append_data(&mut header, name, std::io::empty()).expect("a directory");
}

pub fn link<W: Write>(builder: &mut tar::Builder<W>, name: &str, target: &str) {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Symlink);
    header.set_size(0);
    header.set_mode(0o777);
    builder.append_link(&mut header, name, target).expect("a link");
}

pub fn hard_link<W: Write>(builder: &mut tar::Builder<W>, name: &str, target: &str) {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Link);
    header.set_size(0);
    header.set_mode(0o644);
    builder.append_link(&mut header, name, target).expect("a hard link");
}

pub fn raw_named<W: Write>(builder: &mut tar::Builder<W>, name: &[u8], body: &[u8]) {
    raw(builder, name, tar::EntryType::Regular, 0o644, body);
}

pub fn raw_file<W: Write>(builder: &mut tar::Builder<W>, name: &[u8], body: &[u8], mode: u32) {
    raw(builder, name, tar::EntryType::Regular, mode, body);
}

pub fn raw_directory<W: Write>(builder: &mut tar::Builder<W>, name: &[u8]) {
    raw(builder, name, tar::EntryType::Directory, 0o755, b"");
}

pub fn pax_global_header<W: Write>(builder: &mut tar::Builder<W>, records: &[u8]) {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::XGlobalHeader);
    header.set_size(records.len() as u64);
    header.set_mode(0o666);
    builder.append_data(&mut header, "pax_global_header", records).expect("a global header");
}

pub fn long_name_of<W: Write>(builder: &mut tar::Builder<W>, length: usize) {
    let name = vec![b'a'; length];
    raw(builder, b"././@LongLink", tar::EntryType::GNULongName, 0o644, &name);
}

pub fn padded_file<W: Write>(
    builder: &mut tar::Builder<W>,
    name: &str,
    head: &[u8],
    size: u64,
    mode: u32,
) {
    let mut header = tar::Header::new_gnu();
    header.set_size(size);
    header.set_mode(mode);
    let body = std::io::Read::chain(
        std::io::Cursor::new(head.to_vec()),
        std::io::Read::take(std::io::repeat(0), size - head.len() as u64),
    );
    builder.append_data(&mut header, name, body).expect("an entry");
}

pub fn raw_announcing<W: Write>(
    builder: &mut tar::Builder<W>,
    name: &[u8],
    kind: tar::EntryType,
    announced: u64,
    body: &[u8],
) {
    let mut header = tar::Header::new_gnu();
    header.as_old_mut().name[..name.len()].copy_from_slice(name);
    header.set_entry_type(kind);
    header.set_size(announced);
    header.set_mode(0o644);
    header.set_cksum();
    builder.append(&header, body).expect("an entry");
}

pub fn noise(length: usize) -> Vec<u8> {
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    (0..length)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 24) as u8
        })
        .collect()
}

pub fn declaring<W: Write>(
    builder: &mut tar::Builder<W>,
    name: &str,
    kind: tar::EntryType,
    size: u64,
) {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(kind);
    header.set_size(size);
    header.set_mode(0o644);
    builder.append_data(&mut header, name, std::io::empty()).expect("an entry");
}

fn raw<W: Write>(
    builder: &mut tar::Builder<W>,
    name: &[u8],
    kind: tar::EntryType,
    mode: u32,
    body: &[u8],
) {
    let mut header = tar::Header::new_gnu();
    header.as_old_mut().name[..name.len()].copy_from_slice(name);
    header.set_entry_type(kind);
    header.set_size(body.len() as u64);
    header.set_mode(mode);
    header.set_cksum();
    builder.append(&header, body).expect("an entry");
}

pub fn nothing_is_loose(home: &Path) {
    let mut seen = 0usize;
    for found in walkdir::WalkDir::new(home).follow_links(false) {
        let found = found.expect("every step of the runtime");
        let at = found.path().display();
        if found.file_type().is_symlink() {
            continue;
        }

        seen += 1;
        let mode = found
            .path()
            .symlink_metadata()
            .expect("the mode of every step")
            .permissions()
            .mode()
            & 0o7777;
        assert_eq!(mode & 0o022, 0, "{at} is {mode:o}, which group or others may write");
        assert_eq!(mode & 0o7000, 0, "{at} is {mode:o}, which carries setuid, setgid or sticky");

        if found.file_type().is_dir() {
            assert_eq!(mode, 0o755, "{at} is a directory and is {mode:o}");
        } else {
            assert!(mode == 0o644 || mode == 0o755, "{at} is a file and is {mode:o}");
        }
    }
    assert!(seen > 2, "{} holds nothing worth walking", home.display());
}

static COUNTER: AtomicUsize = AtomicUsize::new(0);

pub struct Scratch(PathBuf);

impl Scratch {
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

pub fn a_data_dir() -> Scratch {
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir()
        .join(format!("craftpanel-java-{}-{unique}", std::process::id()));
    std::fs::create_dir_all(&path).expect("a scratch directory");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("a data directory a game account can walk through");
    Scratch(path)
}
