use std::io::{self, Read, Seek, SeekFrom};

use axum::http::StatusCode;
use serde::Serialize;
use time::OffsetDateTime;

use crate::auth::error::{Failure, Result};
use crate::files::{self, Kind, RelPath, Root};
use crate::model::Timestamp;

pub const DEFAULT_LIMIT: usize = 200;
pub const MAX_LIMIT: usize = 500;

pub const MAX_CONTENT_LINES: usize = 25_000;
pub const MAX_CONTENT_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_UNPACKED_BYTES: u64 = 512 * 1024 * 1024;
pub const ANALYSIS_BYTES: u64 = 2 * 1024 * 1024;

pub const LOGS: &str = "logs";
pub const CRASH_REPORTS: &str = "crash-reports";
pub const LATEST: &str = "logs/latest.log";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LogFileKind {
    Log,
    CrashReport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LogFile {
    pub file: String,
    pub name: String,
    pub kind: LogFileKind,
    pub size_bytes: u64,
    pub modified_at: Timestamp,
    pub compressed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LogFileListResponse {
    pub total: usize,
    pub truncated: bool,
    pub files: Vec<LogFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LogFileContentResponse {
    pub file: String,
    pub size_bytes: u64,
    pub content_bytes: u64,
    pub truncated: bool,
    pub content: String,
}

const SOURCES: [(&str, LogFileKind, &[&str]); 2] = [
    (LOGS, LogFileKind::Log, &[".log", ".log.gz", ".txt"]),
    (CRASH_REPORTS, LogFileKind::CrashReport, &[".txt"]),
];

pub fn list(root: &Root, limit: usize, offset: usize) -> LogFileListResponse {
    let mut found: Vec<(i64, LogFile)> = Vec::new();

    for (directory, kind, endings) in SOURCES {
        let at = RelPath::parse(directory).expect("a directory name of our own");
        let Ok(dir) = root.dir(&at) else { continue };
        let Ok(entries) = dir.entries() else { continue };

        for raw in entries {
            let Ok(name) = std::str::from_utf8(&raw) else { continue };
            let lower = name.to_ascii_lowercase();
            if !endings.iter().any(|ending| lower.ends_with(ending)) {
                continue;
            }
            let Ok(meta) = dir.meta(&raw) else { continue };
            if meta.kind != Kind::File {
                continue;
            }

            found.push((
                meta.modified,
                LogFile {
                    file: format!("{directory}/{name}"),
                    name: name.to_owned(),
                    kind,
                    size_bytes: meta.size,
                    modified_at: moment(meta.modified),
                    compressed: lower.ends_with(".gz"),
                },
            ));
        }
    }

    found.sort_by(|(left, one), (right, other)| {
        (one.file == LATEST)
            .cmp(&(other.file == LATEST))
            .reverse()
            .then(right.cmp(left))
            .then_with(|| one.file.cmp(&other.file))
    });

    let total = found.len();
    let files: Vec<LogFile> =
        found.into_iter().skip(offset).take(limit).map(|(_, file)| file).collect();

    LogFileListResponse { truncated: offset + files.len() < total, total, files }
}

pub fn target(raw: &str) -> Result<RelPath> {
    let at = RelPath::parse(raw)?;
    let inside = matches!(at.segments().first().map(String::as_str), Some(LOGS | CRASH_REPORTS));
    if !inside || at.depth() != 2 {
        return Err(Failure::new(
            StatusCode::FORBIDDEN,
            "forbidden_path",
            "the console reads logs/ and crash-reports/, one level deep",
        ));
    }
    Ok(at)
}

pub fn on_the_wire(at: &RelPath) -> String {
    at.segments().join("/")
}

pub fn read(root: &Root, at: &RelPath) -> Result<LogFileContentResponse> {
    let file = root.open_read(at).map_err(missing)?;
    let size = size_if_regular(&file)?;

    let packed = at.name().is_some_and(|name| name.to_ascii_lowercase().ends_with(".gz"));
    let (bytes, cut) =
        if packed { unpack(file)? } else { tail(file, MAX_CONTENT_BYTES).map_err(io)? };

    let (bytes, cut) = keep_last_lines(bytes, cut);
    let content = String::from_utf8(bytes).map_err(|_| {
        Failure::new(StatusCode::UNPROCESSABLE_ENTITY, "log_not_text", "this file is not UTF-8")
    })?;

    Ok(LogFileContentResponse {
        file: on_the_wire(at),
        size_bytes: size,
        content_bytes: content.len() as u64,
        truncated: cut,
        content,
    })
}

fn size_if_regular(file: &std::fs::File) -> Result<u64> {
    let stat = file.metadata().map_err(io)?;
    if !stat.is_file() {
        return Err(Failure::bad_request("not_a_regular_file", "only plain files can be read"));
    }
    Ok(stat.len())
}

pub fn remove(root: &Root, at: &RelPath) -> Result<()> {
    let name = at.name().ok_or_else(|| Failure::not_found("log_not_found", "no name given"))?;
    let parent = root.parent_of(at).map_err(missing)?;

    match parent.meta(name.as_bytes()).map_err(missing)?.kind {
        Kind::Directory => Err(Failure::not_found("log_not_found", "that is a directory")),
        _ => parent.unlink(name.as_bytes()).map_err(missing),
    }
}

pub fn latest_for_analysis(root: &Root) -> Result<(String, i64, u64)> {
    let at = RelPath::parse(LATEST).expect("a path of our own");
    let file = root.open_read(&at).map_err(|err| files::fault(&err, "log_file_missing"))?;
    let stat = file.metadata().map_err(io)?;
    if !stat.is_file() {
        return Err(Failure::not_found("log_file_missing", "there is no latest.log"));
    }

    let modified = files::jail::seconds_of(stat.modified());
    let (bytes, _) = tail(file, ANALYSIS_BYTES).map_err(io)?;
    Ok((String::from_utf8_lossy(&bytes).into_owned(), modified, stat.len()))
}

pub fn last_bytes(text: &str, most: u64) -> &str {
    if text.len() as u64 <= most {
        return text;
    }
    let mut from = text.len() - most as usize;
    if let Some(newline) = text.as_bytes()[from..].iter().position(|byte| *byte == b'\n') {
        return &text[from + newline + 1..];
    }
    while from < text.len() && !text.is_char_boundary(from) {
        from += 1;
    }
    &text[from..]
}

fn tail(mut file: std::fs::File, most: u64) -> io::Result<(Vec<u8>, bool)> {
    let end = file.seek(SeekFrom::End(0))?;
    let from = end.saturating_sub(most);
    file.seek(SeekFrom::Start(from))?;

    let mut bytes = Vec::new();
    file.take(most).read_to_end(&mut bytes)?;
    if from == 0 {
        return Ok((bytes, false));
    }
    let start = match bytes.iter().position(|byte| *byte == b'\n') {
        Some(newline) => newline + 1,
        None => char_start(&bytes, 0),
    };
    Ok((bytes.split_off(start), true))
}

fn char_start(bytes: &[u8], mut at: usize) -> usize {
    while at < bytes.len() && bytes[at] & 0xC0 == 0x80 {
        at += 1;
    }
    at
}

fn unpack(file: std::fs::File) -> Result<(Vec<u8>, bool)> {
    let mut decoder = flate2::read::GzDecoder::new(file);
    let mut chunk = vec![0u8; 64 * 1024];
    let mut window: Vec<u8> = Vec::new();
    let mut unpacked = 0u64;
    let mut cut = false;

    loop {
        let read = decoder.read(&mut chunk).map_err(|_| unreadable_archive())?;
        if read == 0 {
            break;
        }
        unpacked += read as u64;
        if unpacked > MAX_UNPACKED_BYTES {
            return Err(Failure::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "log_too_large",
                "this archive unpacks to more than the panel will read",
            ));
        }

        window.extend_from_slice(&chunk[..read]);
        if window.len() as u64 > MAX_CONTENT_BYTES {
            let over = window.len() - MAX_CONTENT_BYTES as usize;
            let from = match window[over..].iter().position(|byte| *byte == b'\n') {
                Some(newline) => over + newline + 1,
                None => char_start(&window, over),
            };
            window.drain(..from);
            cut = true;
        }
    }

    Ok((window, cut))
}

fn keep_last_lines(bytes: Vec<u8>, cut: bool) -> (Vec<u8>, bool) {
    let mut seen = 0;
    let mut from = 0;
    for (index, byte) in bytes.iter().enumerate().rev() {
        if *byte != b'\n' {
            continue;
        }
        if index + 1 == bytes.len() {
            continue;
        }
        seen += 1;
        if seen == MAX_CONTENT_LINES {
            from = index + 1;
            break;
        }
    }
    if from == 0 {
        return (bytes, cut);
    }
    (bytes[from..].to_vec(), true)
}

fn moment(seconds: i64) -> Timestamp {
    OffsetDateTime::from_unix_timestamp(seconds)
        .map_or_else(|_| Timestamp::at(OffsetDateTime::UNIX_EPOCH), Timestamp::at)
}

fn missing(err: io::Error) -> Failure {
    files::fault(&err, "log_not_found")
}

fn io(err: io::Error) -> Failure {
    Failure::internal(anyhow::Error::new(err).context("reading a log file"))
}

fn unreadable_archive() -> Failure {
    Failure::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "log_not_text",
        "this file does not unpack; the panel cannot read it as a log",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::testing::Sandbox;
    use std::io::Write;

    fn root(sandbox: &Sandbox) -> Root {
        Root::open(sandbox.server_dir()).expect("the server directory")
    }

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut packer =
            flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        packer.write_all(bytes).expect("packing");
        packer.finish().expect("packing")
    }

    fn age(sandbox: &Sandbox, rel: &str, seconds: i64) {
        let path = sandbox.server_dir().join(rel);
        let when = std::time::SystemTime::UNIX_EPOCH
            + std::time::Duration::from_secs(1_760_000_000 + seconds as u64);
        let stamp = std::fs::FileTimes::new().set_modified(when);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .expect("the file")
            .set_times(stamp)
            .expect("setting the time");
    }

    #[test]
    fn only_log_shaped_files_from_the_two_directories_are_listed() {
        let sandbox = Sandbox::new();
        sandbox.write("logs/latest.log", b"now");
        sandbox.write("logs/2026-08-12-1.log.gz", b"packed");
        sandbox.write("logs/notes.txt", b"text");
        sandbox.write("logs/config.json", b"{}");
        sandbox.write("crash-reports/crash-2026-08-12.txt", b"boom");
        sandbox.write("crash-reports/crash.log", b"not a report");
        sandbox.write("world/level.dat", b"world");
        sandbox.mkdir("logs/old");
        std::os::unix::fs::symlink("/etc/passwd", sandbox.server_dir().join("logs/passwd.log"))
            .expect("a link");

        let listed = list(&root(&sandbox), 200, 0);
        let names: Vec<&str> = listed.files.iter().map(|file| file.file.as_str()).collect();

        assert_eq!(listed.total, 4);
        assert!(names.contains(&"logs/latest.log"));
        assert!(names.contains(&"logs/2026-08-12-1.log.gz"));
        assert!(names.contains(&"logs/notes.txt"));
        assert!(names.contains(&"crash-reports/crash-2026-08-12.txt"));
        assert!(!names.contains(&"logs/passwd.log"), "6.4: a link is skipped, not followed");
        assert!(!names.contains(&"crash-reports/crash.log"), "only .txt is a crash report");
        assert!(!names.iter().any(|name| name.contains("config.json") || name.contains("old")));

        let packed = listed.files.iter().find(|file| file.compressed).expect("the .gz");
        assert_eq!(packed.name, "2026-08-12-1.log.gz");
        assert_eq!(packed.kind, LogFileKind::Log);
        let report = listed.files.iter().find(|file| file.kind == LogFileKind::CrashReport);
        assert!(report.is_some());
    }

    #[test]
    fn latest_log_leads_and_the_rest_follow_by_age() {
        let sandbox = Sandbox::new();
        sandbox.write("logs/latest.log", b"now");
        sandbox.write("logs/old.log", b"old");
        sandbox.write("logs/newer.log", b"newer");
        age(&sandbox, "logs/latest.log", 0);
        age(&sandbox, "logs/old.log", 100);
        age(&sandbox, "logs/newer.log", 200);

        let listed = list(&root(&sandbox), 200, 0);
        let order: Vec<&str> = listed.files.iter().map(|file| file.file.as_str()).collect();
        assert_eq!(order, ["logs/latest.log", "logs/newer.log", "logs/old.log"]);
        assert!(!listed.truncated);
    }

    #[test]
    fn the_page_is_cut_after_the_sorting_and_says_how_much_there_was() {
        let sandbox = Sandbox::new();
        for index in 0..5 {
            sandbox.write(&format!("logs/{index}.log"), b"line");
        }

        let first = list(&root(&sandbox), 2, 0);
        assert_eq!(first.total, 5);
        assert_eq!(first.files.len(), 2);
        assert!(first.truncated);

        let last = list(&root(&sandbox), 2, 4);
        assert_eq!(last.total, 5);
        assert_eq!(last.files.len(), 1);
        assert!(!last.truncated, "offset + files == total is the end of the list");
    }

    #[test]
    fn a_path_outside_the_two_directories_is_refused() {
        for outside in ["server.properties", "world/level.dat", "logs/old/1.log", "logs"] {
            let refused = target(outside).unwrap_err();
            assert_eq!(refused.code(), "forbidden_path", "{outside}");
            assert_eq!(refused.status(), StatusCode::FORBIDDEN);
        }
        for climbing in ["logs/../../panel.db", "../panel.db", "logs/../server.properties"] {
            assert_eq!(target(climbing).unwrap_err().code(), "invalid_path", "{climbing}");
        }

        assert_eq!(target("/logs/latest.log").unwrap().segments(), ["logs", "latest.log"]);
        assert_eq!(target("crash-reports/a.txt").unwrap().depth(), 2);
    }

    #[test]
    fn a_link_that_leaves_the_tree_hands_out_nothing() {
        let sandbox = Sandbox::new();
        let secret = sandbox.data_dir().join("panel.db");
        std::fs::write(&secret, b"password hashes").expect("the panel database");
        sandbox.mkdir("logs");
        std::os::unix::fs::symlink(&secret, sandbox.server_dir().join("logs/latest.log"))
            .expect("the link a plugin may lay");

        let refused = read(&root(&sandbox), &target("logs/latest.log").unwrap()).unwrap_err();
        assert_eq!(refused.code(), "forbidden_path");
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);

        let analysis = latest_for_analysis(&root(&sandbox)).unwrap_err();
        assert_eq!(analysis.code(), "forbidden_path", "6.3 reads the same file");
    }

    #[test]
    fn deleting_a_link_takes_the_link_and_leaves_its_target() {
        let sandbox = Sandbox::new();
        let secret = sandbox.data_dir().join("panel.db");
        std::fs::write(&secret, b"password hashes").expect("the panel database");
        sandbox.mkdir("logs");
        std::os::unix::fs::symlink(&secret, sandbox.server_dir().join("logs/a.log"))
            .expect("the link");

        remove(&root(&sandbox), &target("logs/a.log").unwrap()).expect("the link goes");
        assert!(!sandbox.server_dir().join("logs/a.log").is_symlink());
        assert_eq!(std::fs::read(&secret).unwrap(), b"password hashes");
    }

    #[test]
    fn a_missing_log_is_a_missing_log_and_not_a_server_error() {
        let sandbox = Sandbox::new();
        sandbox.mkdir("logs");
        let missing = read(&root(&sandbox), &target("logs/nothing.log").unwrap()).unwrap_err();
        assert_eq!(missing.code(), "log_not_found");
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);

        let gone = remove(&root(&sandbox), &target("logs/nothing.log").unwrap()).unwrap_err();
        assert_eq!(gone.code(), "log_not_found");
    }

    #[test]
    fn what_is_no_plain_file_is_not_read_and_says_which_rule_that_is() {
        let sandbox = Sandbox::new();
        sandbox.mkdir("logs/old.log");

        let refused = read(&root(&sandbox), &target("logs/old.log").unwrap()).unwrap_err();
        assert_eq!(refused.code(), "not_a_regular_file");
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST);

        let pipe = std::ffi::CString::new(
            sandbox.server_dir().join("logs/pipe.log").into_os_string().into_encoded_bytes(),
        )
        .expect("a path");
        assert_eq!(unsafe { libc::mkfifo(pipe.as_ptr(), 0o600) }, 0, "the fifo a plugin may lay");

        let hanging = read(&root(&sandbox), &target("logs/pipe.log").unwrap()).unwrap_err();
        assert_eq!(hanging.code(), "not_a_regular_file");

        remove(&root(&sandbox), &target("logs/pipe.log").unwrap()).expect("the fifo goes");
    }

    #[test]
    fn a_plain_log_comes_back_whole() {
        let sandbox = Sandbox::new();
        sandbox.write("logs/latest.log", b"[15:04:22] one\n[15:04:23] two\n");

        let answer = read(&root(&sandbox), &target("logs/latest.log").unwrap()).unwrap();
        assert_eq!(answer.file, "logs/latest.log", "no leading slash: the provider compares it");
        assert_eq!(answer.content, "[15:04:22] one\n[15:04:23] two\n");
        assert_eq!(answer.size_bytes, 30);
        assert_eq!(answer.content_bytes, 30);
        assert!(!answer.truncated);
    }

    #[test]
    fn a_gz_is_unpacked_and_its_size_is_the_packed_one() {
        let sandbox = Sandbox::new();
        let plain = "[15:04:22] a line\n".repeat(500);
        let packed = gzip(plain.as_bytes());
        sandbox.write("logs/2026-08-12-1.log.gz", &packed);

        let answer = read(&root(&sandbox), &target("logs/2026-08-12-1.log.gz").unwrap()).unwrap();
        assert_eq!(answer.content, plain);
        assert_eq!(answer.content_bytes, plain.len() as u64);
        assert_eq!(answer.size_bytes, packed.len() as u64);
        assert!(answer.size_bytes < answer.content_bytes);
        assert!(!answer.truncated);
    }

    #[test]
    fn a_file_without_a_single_line_break_is_not_answered_with_nothing() {
        let sandbox = Sandbox::new();
        let mut log = "ä".repeat(MAX_CONTENT_BYTES as usize / 2 + 512);
        log.push_str("the end");
        sandbox.write("logs/latest.log", log.as_bytes());

        let answer = read(&root(&sandbox), &target("logs/latest.log").unwrap()).unwrap();
        assert!(answer.truncated);
        assert!(answer.content.ends_with("the end"));
        assert!(answer.content_bytes > MAX_CONTENT_BYTES - 8, "only {}", answer.content_bytes);
        assert!(answer.content_bytes <= MAX_CONTENT_BYTES);

        let packed = gzip(log.as_bytes());
        sandbox.write("logs/one.log.gz", &packed);
        let unpacked = read(&root(&sandbox), &target("logs/one.log.gz").unwrap()).unwrap();
        assert!(unpacked.truncated);
        assert!(unpacked.content.ends_with("the end"));
        assert!(unpacked.content_bytes > MAX_CONTENT_BYTES - 8, "only {}", unpacked.content_bytes);
    }

    #[test]
    fn a_long_log_loses_its_beginning_and_keeps_its_end() {
        let sandbox = Sandbox::new();
        let mut log = String::from("the first line, which must not survive\n");
        for index in 0..MAX_CONTENT_LINES + 500 {
            log.push_str(&format!("[15:04:22] [Server thread/INFO]: line {index}\n"));
        }
        sandbox.write("logs/latest.log", log.as_bytes());

        let answer = read(&root(&sandbox), &target("logs/latest.log").unwrap()).unwrap();
        assert!(answer.truncated, "6.5: cut at the front, and it has to say so");
        assert_eq!(answer.content.lines().count(), MAX_CONTENT_LINES);
        assert!(!answer.content.contains("must not survive"));
        assert!(answer.content.ends_with(&format!("line {}\n", MAX_CONTENT_LINES + 499)));
        assert!(answer.content.starts_with("[15:04:22]"), "never half a line");
    }

    #[test]
    fn a_gz_that_unfolds_past_the_ceiling_is_refused_instead_of_read() {
        let sandbox = Sandbox::new();
        let line = "[15:04:22] [Server thread/INFO]: a line of a log that repeats forever\n";
        let mut packer =
            flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let block = line.repeat(16_000);
        let mut written = 0u64;
        while written <= MAX_UNPACKED_BYTES {
            packer.write_all(block.as_bytes()).expect("packing");
            written += block.len() as u64;
        }
        let bomb = packer.finish().expect("packing");
        assert!(bomb.len() < 4 * 1024 * 1024, "a small file, {} bytes", bomb.len());
        sandbox.write("logs/bomb.log.gz", &bomb);

        let refused = read(&root(&sandbox), &target("logs/bomb.log.gz").unwrap()).unwrap_err();
        assert_eq!(refused.code(), "log_too_large");
        assert_eq!(refused.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[test]
    fn a_file_that_is_not_text_is_named_as_such() {
        let sandbox = Sandbox::new();
        sandbox.write("logs/broken.log", &[0xff, 0xfe, 0x00, 0x01]);
        let refused = read(&root(&sandbox), &target("logs/broken.log").unwrap()).unwrap_err();
        assert_eq!(refused.code(), "log_not_text");
        assert_eq!(refused.status(), StatusCode::UNPROCESSABLE_ENTITY);

        sandbox.write("logs/half.log.gz", b"this was never gzip");
        let torn = read(&root(&sandbox), &target("logs/half.log.gz").unwrap()).unwrap_err();
        assert_eq!(torn.code(), "log_not_text");
    }

    #[test]
    fn the_analysis_reads_the_end_of_the_log_on_a_line_boundary() {
        let sandbox = Sandbox::new();
        let mut log = String::from("the oldest line\n");
        while log.len() < (ANALYSIS_BYTES as usize) * 2 {
            log.push_str("[15:04:22] [Server thread/WARN]: something happened again\n");
        }
        log.push_str("[15:04:30] [Server thread/ERROR]: the last one\n");
        sandbox.write("logs/latest.log", log.as_bytes());

        let (text, modified, size) = latest_for_analysis(&root(&sandbox)).unwrap();
        assert!(text.len() as u64 <= ANALYSIS_BYTES);
        assert!(text.starts_with("[15:04:22]"), "half a line would parse as an entry");
        assert!(text.ends_with("the last one\n"));
        assert!(!text.contains("the oldest line"));
        assert_eq!(size, log.len() as u64);
        assert!(modified > 0);

        std::fs::remove_file(sandbox.server_dir().join("logs/latest.log")).unwrap();
        let gone = latest_for_analysis(&root(&sandbox)).unwrap_err();
        assert_eq!(gone.code(), "log_file_missing");
        assert_eq!(gone.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn the_last_bytes_of_a_text_begin_at_a_line() {
        let text = "one\ntwo\nthree\n";
        assert_eq!(last_bytes(text, 100), text);
        assert_eq!(last_bytes(text, 10), "three\n");
        assert_eq!(last_bytes("no line breaks at all", 5), "t all");
    }

    #[test]
    fn a_cut_that_lands_inside_a_character_moves_past_it() {
        assert_eq!(last_bytes("aäa", 2), "a");
        assert_eq!(last_bytes("aäa", 3), "äa");

        let mut log = "[15:04:22] [Server thread/INFO]: Renée joined the game\n".repeat(60_000);
        assert!(log.len() as u64 > ANALYSIS_BYTES);
        let cut = last_bytes(&log, ANALYSIS_BYTES);
        assert!(cut.starts_with("[15:04:22]"));
        assert!(cut.len() as u64 <= ANALYSIS_BYTES);

        log.retain(|letter| letter != '\n');
        assert!(!last_bytes(&log, ANALYSIS_BYTES).is_empty());
    }
}
