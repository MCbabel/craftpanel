use std::collections::HashSet;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::{self, Sender};

use crate::auth::error::{Failure, Result};
use crate::model::{Id, OperationError, OperationErrorStep};
use crate::ops::{Operations, Step};

use super::jail::{Kind, Part, Root};
use super::path::RelPath;
use super::{Workspace, MAX_CONFLICTS, MAX_EXTRACT_ENTRIES, MAX_EXTRACT_UNCOMPRESSED_BYTES};

const MAX_RATIO: u64 = 200;
const RATIO_FLOOR: u64 = 64 * 1024 * 1024;
const UNPACK_SHARE: f64 = 0.9;

#[derive(Debug, Clone, Deserialize)]
pub struct ExtractRequest {
    pub path: String,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(rename = "override")]
    pub override_existing: bool,
    pub dry: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractDryRunResponse {
    pub modpack_name: Option<String>,
    pub conflicting_files: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Survey {
    pub entries: u64,
    pub uncompressed: u64,
    pub compressed: u64,
    pub modpack_name: Option<String>,
    pub conflicting_files: Vec<String>,
}

impl Survey {
    pub fn looks_like_a_bomb(&self) -> bool {
        self.uncompressed > RATIO_FLOOR
            && self.uncompressed / self.compressed.max(1) > MAX_RATIO
    }

    pub fn over_the_ceiling(&self) -> bool {
        self.entries > MAX_EXTRACT_ENTRIES || self.uncompressed > MAX_EXTRACT_UNCOMPRESSED_BYTES
    }
}

pub fn survey(root: &Root, archive: &RelPath, target: &RelPath) -> Result<Survey> {
    let name = archive.name().unwrap_or_default();
    if !name.to_ascii_lowercase().ends_with(".zip") {
        return Err(unsupported());
    }
    match root.meta(archive).map_err(|err| super::fault(&err, "not_found"))?.kind {
        Kind::File => {}
        Kind::Directory => return Err(Failure::bad_request("not_a_regular_file", "this is a directory")),
        _ => return Err(Failure::bad_request("not_a_regular_file", "this is not a plain file")),
    }

    let file = root.open_read(archive).map_err(|err| super::fault(&err, "not_found"))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|_| unsupported())?;

    let mut survey = Survey::default();
    for index in 0..zip.len() {
        let Ok(entry) = zip.by_index(index) else {
            return Err(unsupported());
        };
        if entry.is_dir() || entry.is_symlink() {
            continue;
        }

        survey.entries += 1;
        survey.uncompressed = survey.uncompressed.saturating_add(entry.size());
        survey.compressed = survey.compressed.saturating_add(entry.compressed_size());
        if survey.over_the_ceiling() {
            return Err(Failure::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "archive_too_large",
                "this archive would unpack to more than the panel allows",
            ));
        }

        if survey.conflicting_files.len() < MAX_CONFLICTS {
            let raw = entry.name_raw().to_vec();
            if let Some(there) = place(target, &raw) {
                if root.exists(&there) {
                    survey.conflicting_files.push(there.on_the_wire());
                }
            }
        }
    }

    survey.modpack_name = pack_name(&mut zip);
    Ok(survey)
}

fn pack_name<R: Read + std::io::Seek>(zip: &mut zip::ZipArchive<R>) -> Option<String> {
    for candidate in ["modrinth.index.json", "manifest.json"] {
        let Ok(entry) = zip.by_name(candidate) else {
            continue;
        };
        let mut text = String::new();
        if entry.take(1024 * 1024).read_to_string(&mut text).is_err() {
            continue;
        }
        if let Ok(serde_json::Value::Object(fields)) = serde_json::from_str(&text) {
            if let Some(serde_json::Value::String(name)) = fields.get("name") {
                return Some(name.clone());
            }
        }
    }
    None
}

fn place(target: &RelPath, raw: &[u8]) -> Option<RelPath> {
    let inside = RelPath::parse_bytes(raw).ok()?;
    if inside.is_root() {
        return None;
    }
    target.join(&inside).ok()
}

fn unsupported() -> Failure {
    Failure::new(
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "unsupported_archive",
        "only zip archives can be unpacked",
    )
}

pub struct Job {
    pub operations: Arc<Operations>,
    pub workspace: Workspace,
    pub operation: Id,
    pub archive: RelPath,
    pub target: RelPath,
    pub replace: bool,
}

#[derive(Debug)]
enum Halt {
    Cancelled,
    Ended,
    Failed(OperationError),
}

fn failed(code: &str, step: OperationErrorStep, message: impl Into<String>) -> Halt {
    Halt::Failed(OperationError { code: code.to_owned(), message: message.into(), step })
}

pub fn start(job: Job) {
    tokio::spawn(async move {
        let operations = Arc::clone(&job.operations);
        let id = job.operation;
        match run(job).await {
            Ok(()) => {}
            Err(Halt::Cancelled) => {
                let _ = operations.cancelled(id).await;
            }
            Err(Halt::Ended) => {}
            Err(Halt::Failed(error)) => {
                tracing::warn!("unarchive {id} failed: {} — {}", error.code, error.message);
                let _ = operations.fail(id, error).await;
            }
        }
    });
}

async fn run(job: Job) -> std::result::Result<(), Halt> {
    let Job { operations, workspace, operation, archive, target, replace } = job;
    wait_for_a_turn(&operations, operation).await?;

    let work = RelPath::parse(super::WORK_DIR)
        .and_then(|dir| dir.child(&operation.to_string()))
        .map_err(|_| failed("invalid_path", OperationErrorStep::Filesystem, "no work directory"))?;

    let cancel = Arc::new(AtomicBool::new(false));
    let unpacked = {
        let root = clone_root(&workspace)?;
        let (archive, work, watched) = (archive.clone(), work.clone(), Arc::clone(&cancel));
        drive(&operations, operation, &cancel, move |progress| {
            unpack(&root, &archive, &work, &watched, &progress)
        })
        .await?
    };

    let handover = handover(&clone_root(&workspace)?, &target);

    mark_applied(&operations, operation).await;

    let applied = {
        let root = clone_root(&workspace)?;
        let (work, target, watched) = (work.clone(), target.clone(), Arc::clone(&cancel));
        drive(&operations, operation, &cancel, move |progress| {
            apply(&root, &work, &target, replace, unpacked, &watched, &progress)
        })
        .await
    };

    let handed = workspace.hand_back(&handover).await;
    applied?;
    handed.map_err(|failure| {
        failed("chown_failed", OperationErrorStep::Filesystem, failure.to_string())
    })?;

    let root = clone_root(&workspace)?;
    let sweep = work.clone();
    let _ = tokio::task::spawn_blocking(move || {
        let parent = root.dir(&sweep.parent()).ok()?;
        parent.remove_tree(sweep.name()?.as_bytes()).ok()
    })
    .await;

    if unpacked.unusable > 0 {
        return Err(failed(
            "invalid_path",
            OperationErrorStep::Filesystem,
            format!("{} entries had names that cannot be used", unpacked.unusable),
        ));
    }

    let _ = operations
        .advance(operation, Step { progress: Some(1.0), ..Step::default() })
        .await;
    let _ = operations.finish(operation).await;
    Ok(())
}

async fn wait_for_a_turn(operations: &Operations, id: Id) -> std::result::Result<(), Halt> {
    loop {
        if operations.cancel_requested(id).await.unwrap_or(false) {
            return Err(Halt::Cancelled);
        }
        match operations.begin(id).await {
            Ok(Some(_)) => return Ok(()),
            Ok(None) => {}
            Err(fault) => {
                return Err(failed(
                    "interrupted_while_applying",
                    OperationErrorStep::Filesystem,
                    fault.message().to_owned(),
                ))
            }
        }

        match operations.get(id).await {
            Ok(run) if run.state != crate::model::OperationState::Queued => {
                return Err(Halt::Ended)
            }
            Err(_) => return Err(Halt::Ended),
            _ => {}
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn drive<T, F>(
    operations: &Operations,
    id: Id,
    cancel: &AtomicBool,
    work: F,
) -> std::result::Result<T, Halt>
where
    T: Send + 'static,
    F: FnOnce(Sender<Step>) -> std::result::Result<T, Halt> + Send + 'static,
{
    let (sender, mut steps) = mpsc::channel::<Step>(8);
    let mut worker = tokio::task::spawn_blocking(move || work(sender));
    let mut tick = tokio::time::interval(Duration::from_millis(400));

    let outcome = loop {
        tokio::select! {
            step = steps.recv() => match step {
                Some(step) => { let _ = operations.advance(id, step).await; }
                None => break (&mut worker).await,
            },
            _ = tick.tick() => {
                if operations.cancel_requested(id).await.unwrap_or(false) {
                    cancel.store(true, Ordering::Relaxed);
                }
            }
        }
    };

    match outcome {
        Ok(result) => result,
        Err(err) => Err(failed(
            "interrupted_while_applying",
            OperationErrorStep::Filesystem,
            format!("the unpacking task died: {err}"),
        )),
    }
}

fn handover(root: &Root, target: &RelPath) -> RelPath {
    let mut here = RelPath::root();
    for segment in target.segments() {
        let next = here.with_name(segment);
        if !root.exists(&next) {
            return next;
        }
        here = next;
    }
    target.clone()
}

fn clone_root(workspace: &Workspace) -> std::result::Result<Root, Halt> {
    workspace.root().try_clone().map_err(|err| {
        failed("interrupted_while_applying", OperationErrorStep::Filesystem, err.to_string())
    })
}

#[derive(Debug, Clone, Copy, Default)]
struct Unpacked {
    files: u64,
    bytes: u64,
    unusable: u64,
    refused: u64,
}

fn unpack(
    root: &Root,
    archive: &RelPath,
    work: &RelPath,
    cancel: &AtomicBool,
    progress: &Sender<Step>,
) -> std::result::Result<Unpacked, Halt> {
    let file = root
        .open_read(archive)
        .map_err(|err| failed("archive_corrupted", OperationErrorStep::Filesystem, err.to_string()))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|err| failed("archive_corrupted", OperationErrorStep::Filesystem, err.to_string()))?;

    let mut weighed = Survey::default();
    for index in 0..zip.len() {
        if let Ok(entry) = zip.by_index(index) {
            if entry.is_dir() || entry.is_symlink() {
                continue;
            }
            weighed.entries += 1;
            weighed.uncompressed = weighed.uncompressed.saturating_add(entry.size());
            weighed.compressed = weighed.compressed.saturating_add(entry.compressed_size());
        }
    }
    if weighed.looks_like_a_bomb() {
        return Err(failed(
            "archive_corrupted",
            OperationErrorStep::Filesystem,
            "this archive unpacks to far more than it weighs",
        ));
    }
    if weighed.over_the_ceiling() {
        return Err(failed(
            "no_space",
            OperationErrorStep::Filesystem,
            "this archive would unpack to more than the panel allows",
        ));
    }
    let expected = weighed.uncompressed;

    let mut made: HashSet<String> = HashSet::new();
    lay_out(root, work, &mut made)?;

    let mut done = Unpacked::default();
    for index in 0..zip.len() {
        if cancel.load(Ordering::Relaxed) {
            return Err(Halt::Cancelled);
        }

        let mut entry = zip
            .by_index(index)
            .map_err(|err| failed("archive_corrupted", OperationErrorStep::Filesystem, err.to_string()))?;
        let raw = entry.name_raw().to_vec();
        let size = entry.size();
        let directory = entry.is_dir();
        let link = entry.is_symlink();

        let Some(inside) = place(&RelPath::root(), &raw) else {
            done.unusable += 1;
            continue;
        };
        if link {
            done.refused += 1;
            continue;
        }

        let Ok(there) = work.join(&inside) else {
            done.unusable += 1;
            continue;
        };
        if directory {
            lay_out(root, &there, &mut made)?;
            continue;
        }

        lay_out(root, &there.parent(), &mut made)?;
        let dir = root
            .dir(&there.parent())
            .map_err(|err| trouble(&err, "the work directory went away"))?;
        let name = there.name().unwrap_or_default().to_owned();
        let (part, mut out) =
            Part::create(dir, &name).map_err(|err| trouble(&err, "no part file"))?;

        let mut limited = (&mut entry).take(size.saturating_add(1));
        let written = std::io::copy(&mut limited, &mut out)
            .map_err(|err| trouble(&err, "the entry could not be written"))?;
        if written > size {
            return Err(failed(
                "archive_corrupted",
                OperationErrorStep::Filesystem,
                "an entry is bigger than the archive says",
            ));
        }
        out.sync_all().map_err(|err| trouble(&err, "the entry could not be synced"))?;
        drop(out);
        part.commit(name.as_bytes(), true).map_err(|err| trouble(&err, "the entry stayed a part"))?;

        done.files += 1;
        done.bytes = done.bytes.saturating_add(written);
        if done.files % 32 == 0 || written > 4 * 1024 * 1024 {
            let _ = progress.blocking_send(Step {
                progress: Some(share(done.bytes, expected) * UNPACK_SHARE),
                bytes_processed: Some(done.bytes),
                files_processed: Some(done.files),
                current_file: Some(inside.on_the_wire()),
                ..Step::default()
            });
        }
    }

    Ok(done)
}

fn apply(
    root: &Root,
    work: &RelPath,
    target: &RelPath,
    replace: bool,
    unpacked: Unpacked,
    cancel: &AtomicBool,
    progress: &Sender<Step>,
) -> std::result::Result<(), Halt> {
    let mut made = HashSet::new();
    lay_out(root, target, &mut made)?;
    let mut moved = 0u64;
    move_tree(root, work, target, replace, cancel, progress, &mut moved, unpacked, 0)
}

#[allow(clippy::too_many_arguments)]
fn move_tree(
    root: &Root,
    from: &RelPath,
    into: &RelPath,
    replace: bool,
    cancel: &AtomicBool,
    progress: &Sender<Step>,
    moved: &mut u64,
    unpacked: Unpacked,
    depth: usize,
) -> std::result::Result<(), Halt> {
    if depth > super::path::MAX_DEPTH {
        return Err(failed(
            "invalid_path",
            OperationErrorStep::Filesystem,
            "the archive nests deeper than a path may go",
        ));
    }

    let source = root.dir(from).map_err(|err| trouble(&err, "the work directory went away"))?;
    let sink = root.dir(into).map_err(|err| trouble(&err, "the target went away"))?;
    let names = source.entries().map_err(|err| trouble(&err, "the work directory is unreadable"))?;

    for raw in names {
        if cancel.load(Ordering::Relaxed) {
            return Err(Halt::Cancelled);
        }
        let name = String::from_utf8_lossy(&raw).into_owned();
        let here = from.with_name(&name);
        let there = into.with_name(&name);
        let kind = match source.meta(&raw) {
            Ok(meta) => meta.kind,
            Err(_) => continue,
        };

        if kind == Kind::Directory {
            match sink.ensure_dir(&raw) {
                Ok(()) => {}
                Err(err) => return Err(trouble(&err, "a directory could not be made")),
            }
            move_tree(root, &here, &there, replace, cancel, progress, moved, unpacked, depth + 1)?;
            continue;
        }

        match source.rename_to(&raw, &sink, &raw, replace) {
            Ok(()) => {}
            Err(err) if err.raw_os_error() == Some(libc::EEXIST) => {
                return Err(failed(
                    "already_exists",
                    OperationErrorStep::Filesystem,
                    format!("{} is already there and override was not asked for", there),
                ))
            }
            Err(err) => return Err(trouble(&err, "an entry could not be moved into place")),
        }

        *moved += 1;
        if *moved % 32 == 0 {
            let _ = progress.blocking_send(Step {
                progress: Some(UNPACK_SHARE + share(*moved, unpacked.files) * (1.0 - UNPACK_SHARE)),
                files_processed: Some(*moved),
                current_file: Some(there.on_the_wire()),
                ..Step::default()
            });
        }
    }

    Ok(())
}

fn lay_out(
    root: &Root,
    rel: &RelPath,
    made: &mut HashSet<String>,
) -> std::result::Result<(), Halt> {
    if rel.is_root() || !made.insert(rel.on_the_wire()) {
        return Ok(());
    }

    let mut here = root
        .dir(&RelPath::root())
        .map_err(|err| trouble(&err, "the server directory went away"))?;
    for segment in rel.segments() {
        here.ensure_dir(segment.as_bytes())
            .map_err(|err| trouble(&err, "a directory could not be made"))?;
        here = here
            .child(segment.as_bytes())
            .map_err(|err| trouble(&err, "a directory could not be opened"))?;
    }
    Ok(())
}

fn trouble(err: &std::io::Error, doing: &str) -> Halt {
    let code = match err.raw_os_error() {
        Some(libc::ENOSPC) | Some(libc::EDQUOT) | Some(libc::EFBIG) => "no_space",
        Some(libc::EXDEV) | Some(libc::ELOOP) => "invalid_path",
        _ => "interrupted_while_applying",
    };
    failed(code, OperationErrorStep::Filesystem, format!("{doing}: {err}"))
}

fn share(done: u64, total: u64) -> f64 {
    if total == 0 {
        return 1.0;
    }
    (done as f64 / total as f64).clamp(0.0, 1.0)
}

async fn mark_applied(operations: &Operations, id: Id) {
    let written = sqlx::query("UPDATE operations SET applied_at = ? WHERE id = ?")
        .bind(crate::model::Timestamp::now())
        .bind(id)
        .execute(operations.pool())
        .await;
    if let Err(err) = written {
        tracing::error!("unarchive {id} could not mark the moving half: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::testing::Sandbox;
    use std::io::Write;

    fn zip_with(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buffer = std::io::Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options = zip::write::SimpleFileOptions::default();
        for (name, bytes) in entries {
            if name.ends_with('/') {
                writer.add_directory(*name, options).expect("a directory");
            } else {
                writer.start_file(*name, options).expect("an entry");
                writer.write_all(bytes).expect("the bytes");
            }
        }
        writer.finish().expect("the archive");
        buffer.into_inner()
    }

    fn root(sandbox: &Sandbox) -> Root {
        Root::open(sandbox.server_dir()).expect("the root opens")
    }

    fn at(raw: &str) -> RelPath {
        RelPath::parse(raw).expect("a usable path")
    }

    #[test]
    fn a_dry_run_names_what_would_be_overwritten_and_makes_no_operation() {
        let sandbox = Sandbox::new();
        sandbox.write(
            "plugins/pack.zip",
            &zip_with(&[
                ("config/one.yml", b"new"),
                ("config/two.yml", b"new"),
                ("modrinth.index.json", br#"{"name":"Cobblemon Official"}"#),
            ]),
        );
        sandbox.write("plugins/config/one.yml", b"old");

        let found = survey(&root(&sandbox), &at("/plugins/pack.zip"), &at("/plugins")).unwrap();
        assert_eq!(found.conflicting_files, ["/plugins/config/one.yml"]);
        assert_eq!(found.modpack_name.as_deref(), Some("Cobblemon Official"));
        assert_eq!(found.entries, 3);
        assert_eq!(
            std::fs::read_to_string(sandbox.server_dir().join("plugins/config/one.yml")).unwrap(),
            "old",
            "a dry run writes nothing"
        );
    }

    #[test]
    fn only_a_zip_is_an_archive() {
        let sandbox = Sandbox::new();
        sandbox.write("mods/mod.jar", &zip_with(&[("a", b"a")]));
        sandbox.write("world.tar", b"not a zip at all");
        sandbox.write("broken.zip", b"neither is this");

        let root = root(&sandbox);
        for path in ["/mods/mod.jar", "/world.tar", "/broken.zip"] {
            let refused = survey(&root, &at(path), &RelPath::root()).expect_err("no unpacking");
            assert_eq!(refused.code(), "unsupported_archive", "{path}");
            assert_eq!(refused.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
        }
    }

    #[test]
    fn an_entry_that_climbs_out_lands_nowhere() {
        assert_eq!(place(&at("/plugins"), b"../../etc/passwd"), None);
        assert_eq!(place(&at("/plugins"), b"/etc/passwd").map(|p| p.on_the_wire()),
            Some("/plugins/etc/passwd".to_owned()), "a leading slash means nothing (7.1)");
        assert_eq!(place(&at("/plugins"), b"a\0b"), None);
        assert_eq!(place(&at("/plugins"), &[0xff, 0xfe]), None);
        assert_eq!(place(&RelPath::root(), b"mods/a.jar").map(|p| p.on_the_wire()),
            Some("/mods/a.jar".to_owned()));
    }

    #[test]
    fn a_bomb_is_recognised_from_the_central_directory() {
        let small = Survey { uncompressed: 1024, compressed: 1, ..Survey::default() };
        assert!(!small.looks_like_a_bomb(), "the ratio only counts above 64 MiB of output");

        let bomb = Survey {
            uncompressed: RATIO_FLOOR + 1,
            compressed: (RATIO_FLOOR + 1) / (MAX_RATIO + 1),
            ..Survey::default()
        };
        assert!(bomb.looks_like_a_bomb());

        let honest = Survey { uncompressed: RATIO_FLOOR * 2, compressed: RATIO_FLOOR, ..Survey::default() };
        assert!(!honest.looks_like_a_bomb());
    }

    #[test]
    fn the_two_ceilings_of_check_three_are_the_same_ones_before_and_during_a_run() {
        let ordinary = Survey { entries: 12, uncompressed: 4096, ..Survey::default() };
        assert!(!ordinary.over_the_ceiling());

        let too_many = Survey { entries: MAX_EXTRACT_ENTRIES + 1, ..Survey::default() };
        assert!(too_many.over_the_ceiling());

        let too_big =
            Survey { uncompressed: MAX_EXTRACT_UNCOMPRESSED_BYTES + 1, ..Survey::default() };
        assert!(too_big.over_the_ceiling());
    }

    #[test]
    fn unpacking_writes_into_the_work_directory_and_moving_puts_it_in_place() {
        let sandbox = Sandbox::new();
        sandbox.write(
            "pack.zip",
            &zip_with(&[
                ("mods/", b""),
                ("mods/sodium.jar", b"jar bytes"),
                ("config/deep/one.yml", b"a: 1"),
                ("../escape.txt", b"nope"),
            ]),
        );

        let root = root(&sandbox);
        let work = at("/.craftpanel-tmp/01ARZ3NDEKTSV4RRFFQ69G5FAV");
        let cancel = AtomicBool::new(false);
        let (sender, _receiver) = mpsc::channel(64);

        let done = match unpack(&root, &at("/pack.zip"), &work, &cancel, &sender) {
            Ok(done) => done,
            Err(_) => panic!("the unpacking has to get through"),
        };
        assert_eq!(done.files, 2);
        assert_eq!(done.unusable, 1, "the climbing entry is skipped, not written");

        let unpacked = sandbox.server_dir().join(".craftpanel-tmp/01ARZ3NDEKTSV4RRFFQ69G5FAV");
        assert_eq!(std::fs::read_to_string(unpacked.join("mods/sodium.jar")).unwrap(), "jar bytes");
        assert!(!sandbox.server_dir().join("mods").exists(), "nothing in the server tree yet");
        assert!(!sandbox.data_dir().join("escape.txt").exists());

        if apply(&root, &work, &RelPath::root(), true, done, &cancel, &sender).is_err() {
            panic!("the moving has to get through");
        }
        assert_eq!(
            std::fs::read_to_string(sandbox.server_dir().join("mods/sodium.jar")).unwrap(),
            "jar bytes"
        );
        assert_eq!(
            std::fs::read_to_string(sandbox.server_dir().join("config/deep/one.yml")).unwrap(),
            "a: 1"
        );
    }

    #[test]
    fn an_entry_that_nests_too_deep_is_skipped_like_any_other_bad_name() {
        let sandbox = Sandbox::new();
        let deep = format!("{}x.txt", "a/".repeat(63));
        sandbox.write(
            "pack.zip",
            &zip_with(&[(deep.as_str(), b"too deep"), ("mods/sodium.jar", b"jar bytes")]),
        );

        let root = root(&sandbox);
        let work = at("/.craftpanel-tmp/01ARZ3NDEKTSV4RRFFQ69G5FB0");
        let cancel = AtomicBool::new(false);
        let (sender, _receiver) = mpsc::channel(64);

        let done = match unpack(&root, &at("/pack.zip"), &work, &cancel, &sender) {
            Ok(done) => done,
            Err(_) => panic!("one unusable name must not take the whole run with it"),
        };
        assert_eq!(done.unusable, 1);
        assert_eq!(done.files, 1, "the entry behind it is still unpacked");
        assert!(sandbox
            .server_dir()
            .join(".craftpanel-tmp/01ARZ3NDEKTSV4RRFFQ69G5FB0/mods/sodium.jar")
            .exists());
    }

    #[test]
    fn moving_without_override_stops_at_the_first_conflict() {
        let sandbox = Sandbox::new();
        sandbox.write("pack.zip", &zip_with(&[("keep.txt", b"from the archive")]));
        sandbox.write("keep.txt", b"mine");

        let root = root(&sandbox);
        let work = at("/.craftpanel-tmp/01ARZ3NDEKTSV4RRFFQ69G5FAW");
        let cancel = AtomicBool::new(false);
        let (sender, _receiver) = mpsc::channel(64);

        let done = unpack(&root, &at("/pack.zip"), &work, &cancel, &sender).ok().expect("unpacked");
        match apply(&root, &work, &RelPath::root(), false, done, &cancel, &sender) {
            Err(Halt::Failed(error)) => assert_eq!(error.code, "already_exists"),
            _ => panic!("a conflict has to stop the run"),
        }
        assert_eq!(std::fs::read_to_string(sandbox.server_dir().join("keep.txt")).unwrap(), "mine");
    }

    #[test]
    fn the_handover_starts_at_the_topmost_directory_the_run_has_to_make() {
        let sandbox = Sandbox::new();
        sandbox.mkdir("plugins/config");
        let root = root(&sandbox);

        assert_eq!(handover(&root, &at("/plugins/config")).on_the_wire(), "/plugins/config");
        assert_eq!(
            handover(&root, &at("/plugins/config/deep/deeper")).on_the_wire(),
            "/plugins/config/deep",
            "chowning only the target would leave its parents belonging to the panel"
        );
        assert_eq!(handover(&root, &at("/brand/new/tree")).on_the_wire(), "/brand");
        assert_eq!(handover(&root, &RelPath::root()), RelPath::root());
    }

    #[tokio::test]
    async fn a_waiting_run_that_was_called_off_stops_asking() {
        let pool = crate::auth::harness::test_pool().await;
        let owner = crate::auth::harness::a_user(&pool, "max").await;
        let server = crate::auth::harness::a_server(&pool, owner, "one", 2048).await;
        let operations = Operations::new(pool, std::env::temp_dir());

        let kind = crate::model::OperationKind::Unarchive;
        let running = operations
            .create(crate::ops::NewOperation::new(server, kind, None))
            .await
            .expect("a first run");
        operations.begin(running.id).await.expect("no fault").expect("it starts");
        let waiting = operations
            .create(crate::ops::NewOperation::new(server, kind, None))
            .await
            .expect("a second run, which has to wait");

        operations.cancelled(waiting.id).await.expect("called off while it waited");

        let stopped =
            tokio::time::timeout(Duration::from_secs(2), wait_for_a_turn(&operations, waiting.id))
                .await;
        match stopped {
            Ok(Err(Halt::Ended)) => {}
            Ok(other) => panic!("a run that is over is not ours to end again: {:?}", other.is_ok()),
            Err(_) => panic!("the worker is still polling a run that ended two seconds ago"),
        }
    }

    #[test]
    fn a_cancelled_run_stops_where_it_stands() {
        let sandbox = Sandbox::new();
        let many: Vec<(String, Vec<u8>)> =
            (0..64).map(|i| (format!("file-{i:02}.txt"), vec![b'x'; 1024])).collect();
        let borrowed: Vec<(&str, &[u8])> =
            many.iter().map(|(name, bytes)| (name.as_str(), bytes.as_slice())).collect();
        sandbox.write("pack.zip", &zip_with(&borrowed));

        let root = root(&sandbox);
        let cancel = AtomicBool::new(true);
        let (sender, _receiver) = mpsc::channel(64);
        let work = at("/.craftpanel-tmp/01ARZ3NDEKTSV4RRFFQ69G5FAX");

        match unpack(&root, &at("/pack.zip"), &work, &cancel, &sender) {
            Err(Halt::Cancelled) => {}
            _ => panic!("a cancel has to be seen"),
        }
    }
}
