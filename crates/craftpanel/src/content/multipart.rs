use std::path::{Path, PathBuf};

use futures::StreamExt;
use tokio::io::AsyncWriteExt;

const TEXT_LIMIT: usize = 64 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum Fault {
    #[error("this endpoint reads multipart/form-data")]
    NotMultipart,
    #[error("the upload is malformed: {0}")]
    Malformed(&'static str),
    #[error("the upload is larger than this panel allows")]
    TooLarge,
    #[error("that file name cannot be used")]
    BadName,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl Fault {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotMultipart => "unsupported_media_type",
            Self::Malformed(_) | Self::BadName => "invalid_request",
            Self::TooLarge => "file_too_large",
            Self::Io(_) => "internal",
        }
    }
}

pub type Result<T> = std::result::Result<T, Fault>;

#[derive(Debug)]
pub enum Content {
    Text(String),
    File { path: PathBuf, size: u64 },
}

#[derive(Debug)]
pub struct Part {
    pub name: String,
    pub file_name: Option<String>,
    pub body: Content,
}

pub fn boundary_of(content_type: &str) -> Result<String> {
    let (kind, rest) = content_type.split_once(';').ok_or(Fault::NotMultipart)?;
    if !kind.trim().eq_ignore_ascii_case("multipart/form-data") {
        return Err(Fault::NotMultipart);
    }
    for parameter in rest.split(';') {
        let Some((key, value)) = parameter.split_once('=') else { continue };
        if key.trim().eq_ignore_ascii_case("boundary") {
            let value = value.trim().trim_matches('"');
            if value.is_empty() || value.len() > 70 {
                return Err(Fault::NotMultipart);
            }
            return Ok(value.to_owned());
        }
    }
    Err(Fault::NotMultipart)
}

pub fn safe_file_name(raw: &str) -> Result<String> {
    let name = raw.rsplit(['/', '\\']).next().unwrap_or(raw).trim();
    let usable = !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('\0')
        && name.len() <= super::paths::MAX_SEGMENT;
    usable.then(|| name.to_owned()).ok_or(Fault::BadName)
}

pub async fn collect(
    boundary: &str,
    body: axum::body::Body,
    into: &Path,
    limit: u64,
) -> Result<Vec<Part>> {
    tokio::fs::create_dir_all(into).await?;

    let delimiter = format!("\r\n--{boundary}").into_bytes();
    let mut stream = body.into_data_stream();
    let mut buffer: Vec<u8> = b"\r\n".to_vec();
    let mut finished = false;

    let mut parts = Vec::new();
    let mut total = 0u64;

    seek(&mut buffer, &delimiter, &mut stream, &mut finished, None, &mut total, limit).await?;

    loop {
        fill_to(&mut buffer, 2, &mut stream, &mut finished).await?;
        if buffer.starts_with(b"--") {
            return Ok(parts);
        }
        let after = skip_line_break(&buffer).ok_or(Fault::Malformed("no line after a boundary"))?;
        buffer.drain(..after);

        let headers = read_headers(&mut buffer, &mut stream, &mut finished).await?;
        let (name, file_name) = disposition(&headers)?;

        match file_name {
            Some(file_name) => {
                let path = into.join(format!("part-{}", crate::model::Id::new()));
                let mut file = tokio::fs::File::create(&path).await?;
                let size = seek(
                    &mut buffer,
                    &delimiter,
                    &mut stream,
                    &mut finished,
                    Some(&mut file),
                    &mut total,
                    limit,
                )
                .await?;
                file.sync_all().await?;
                parts.push(Part { name, file_name: Some(file_name), body: Content::File { path, size } });
            }
            None => {
                let mut text = Vec::new();
                seek_into_memory(
                    &mut buffer,
                    &delimiter,
                    &mut stream,
                    &mut finished,
                    &mut text,
                    &mut total,
                    limit,
                )
                .await?;
                let text = String::from_utf8(text)
                    .map_err(|_| Fault::Malformed("a field that is not UTF-8"))?;
                parts.push(Part { name, file_name: None, body: Content::Text(text) });
            }
        }
    }
}

pub trait Chunks:
    futures::Stream<Item = std::result::Result<axum::body::Bytes, axum::Error>> + Unpin
{
}

impl<T> Chunks for T where
    T: futures::Stream<Item = std::result::Result<axum::body::Bytes, axum::Error>> + Unpin
{
}

async fn seek(
    buffer: &mut Vec<u8>,
    delimiter: &[u8],
    stream: &mut impl Chunks,
    finished: &mut bool,
    mut sink: Option<&mut tokio::fs::File>,
    total: &mut u64,
    limit: u64,
) -> Result<u64> {
    let mut written = 0u64;
    loop {
        match scan(buffer, delimiter, *finished) {
            Search::Found(at) => {
                if let Some(file) = sink.as_deref_mut() {
                    file.write_all(&buffer[..at]).await?;
                }
                written += at as u64;
                buffer.drain(..at + delimiter.len());
                return Ok(written);
            }
            Search::Flush(upto) => {
                if upto > 0 {
                    if let Some(file) = sink.as_deref_mut() {
                        file.write_all(&buffer[..upto]).await?;
                    }
                    written += upto as u64;
                    *total += upto as u64;
                    if *total > limit {
                        return Err(Fault::TooLarge);
                    }
                    buffer.drain(..upto);
                }
                if !pull(buffer, stream, finished).await? {
                    return Err(Fault::Malformed("the body ended inside a part"));
                }
            }
        }
    }
}

enum Search {
    Found(usize),
    Flush(usize),
}

fn scan(buffer: &[u8], delimiter: &[u8], finished: bool) -> Search {
    let mut from = 0;
    while let Some(offset) = find(&buffer[from..], delimiter) {
        let at = from + offset;
        let after = at + delimiter.len();
        if buffer.len() < after + 2 {
            return if finished { Search::Found(at) } else { Search::Flush(at) };
        }
        if buffer[after..].starts_with(b"--") || skip_line_break(&buffer[after..]).is_some() {
            return Search::Found(at);
        }
        from = at + 1;
    }
    Search::Flush(buffer.len().saturating_sub(delimiter.len() + 1))
}

async fn seek_into_memory(
    buffer: &mut Vec<u8>,
    delimiter: &[u8],
    stream: &mut impl Chunks,
    finished: &mut bool,
    out: &mut Vec<u8>,
    total: &mut u64,
    limit: u64,
) -> Result<()> {
    loop {
        match scan(buffer, delimiter, *finished) {
            Search::Found(at) => {
                out.extend_from_slice(&buffer[..at]);
                buffer.drain(..at + delimiter.len());
                return if out.len() > TEXT_LIMIT { Err(Fault::TooLarge) } else { Ok(()) };
            }
            Search::Flush(upto) => {
                if upto > 0 {
                    out.extend_from_slice(&buffer[..upto]);
                    *total += upto as u64;
                    if out.len() > TEXT_LIMIT || *total > limit {
                        return Err(Fault::TooLarge);
                    }
                    buffer.drain(..upto);
                }
                if !pull(buffer, stream, finished).await? {
                    return Err(Fault::Malformed("the body ended inside a field"));
                }
            }
        }
    }
}

async fn read_headers(
    buffer: &mut Vec<u8>,
    stream: &mut impl Chunks,
    finished: &mut bool,
) -> Result<String> {
    loop {
        if let Some(at) = find(buffer, b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&buffer[..at]).into_owned();
            buffer.drain(..at + 4);
            return Ok(headers);
        }
        if buffer.len() > 8 * 1024 {
            return Err(Fault::Malformed("a part header without an end"));
        }
        if !pull(buffer, stream, finished).await? {
            return Err(Fault::Malformed("the body ended inside a part header"));
        }
    }
}

async fn fill_to(
    buffer: &mut Vec<u8>,
    least: usize,
    stream: &mut impl Chunks,
    finished: &mut bool,
) -> Result<()> {
    while buffer.len() < least {
        if !pull(buffer, stream, finished).await? {
            return Err(Fault::Malformed("the body ended after a boundary"));
        }
    }
    Ok(())
}

async fn pull(
    buffer: &mut Vec<u8>,
    stream: &mut impl Chunks,
    finished: &mut bool,
) -> Result<bool> {
    if *finished {
        return Ok(false);
    }
    match stream.next().await {
        Some(Ok(chunk)) => {
            buffer.extend_from_slice(&chunk);
            Ok(true)
        }
        Some(Err(_)) => Err(Fault::Malformed("the connection broke")),
        None => {
            *finished = true;
            Ok(false)
        }
    }
}

fn skip_line_break(buffer: &[u8]) -> Option<usize> {
    let mut at = 0;
    while matches!(buffer.get(at), Some(b' ' | b'\t')) {
        at += 1;
    }
    match buffer.get(at..at + 2) {
        Some(b"\r\n") => Some(at + 2),
        _ => None,
    }
}

fn disposition(headers: &str) -> Result<(String, Option<String>)> {
    let line = headers
        .lines()
        .find(|line| line.to_ascii_lowercase().starts_with("content-disposition:"))
        .ok_or(Fault::Malformed("a part without a content-disposition"))?;

    let name = parameter(line, "name").ok_or(Fault::Malformed("a part without a name"))?;
    let file_name = parameter(line, "filename").map(|raw| safe_file_name(&raw)).transpose()?;
    Ok((name, file_name))
}

fn parameter(line: &str, key: &str) -> Option<String> {
    let mut quoted = false;
    let mut starts = Vec::new();
    for (at, character) in line.char_indices() {
        match character {
            '"' => quoted = !quoted,
            ';' if !quoted => starts.push(at + 1),
            _ => {}
        }
    }

    for start in starts {
        let rest = line[start..].trim_start();
        if !rest.to_ascii_lowercase().starts_with(&format!("{key}=")) {
            continue;
        }
        let value = &rest[key.len() + 1..];
        return Some(match value.strip_prefix('"') {
            Some(inside) => inside.split('"').next().unwrap_or_default().to_owned(),
            None => value.split(';').next().unwrap_or_default().trim().to_owned(),
        });
    }
    None
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOUNDARY: &str = "----craftpanelTest";

    fn body(parts: &[(&str, Option<&str>, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        for (name, file_name, content) in parts {
            out.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
            let disposition = match file_name {
                Some(file_name) => format!(
                    "Content-Disposition: form-data; name=\"{name}\"; filename=\"{file_name}\"\r\n\
                     Content-Type: application/java-archive\r\n\r\n"
                ),
                None => format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n"),
            };
            out.extend_from_slice(disposition.as_bytes());
            out.extend_from_slice(content);
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
        out
    }

    fn in_chunks(bytes: Vec<u8>, size: usize) -> axum::body::Body {
        let chunks: Vec<std::result::Result<Vec<u8>, std::io::Error>> =
            bytes.chunks(size).map(|chunk| Ok(chunk.to_vec())).collect();
        axum::body::Body::from_stream(futures::stream::iter(chunks))
    }

    struct Dir(PathBuf);

    impl Dir {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("craftpanel-mp-{}", crate::model::Id::new())))
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_boundary_is_read_quoted_or_bare_and_nothing_else_is_accepted() {
        assert_eq!(
            boundary_of("multipart/form-data; boundary=\"abc\"").unwrap(),
            "abc"
        );
        assert_eq!(boundary_of("multipart/form-data; boundary=abc").unwrap(), "abc");
        assert!(boundary_of("application/json").is_err());
        assert!(boundary_of("multipart/form-data").is_err());
    }

    #[test]
    fn a_file_name_from_the_wire_never_carries_a_directory() {
        assert_eq!(safe_file_name("mod.jar").unwrap(), "mod.jar");
        assert_eq!(safe_file_name("C:\\Users\\me\\mod.jar").unwrap(), "mod.jar");
        assert_eq!(safe_file_name("../../etc/passwd").unwrap(), "passwd");
        assert!(safe_file_name("..").is_err());
        assert!(safe_file_name("   ").is_err());
        assert!(safe_file_name("a\0b").is_err());
    }

    #[tokio::test]
    async fn two_files_and_a_field_come_back_whole() {
        let dir = Dir::new();
        let raw = body(&[
            ("file", Some("one.jar"), b"the first jar"),
            ("meta", None, br#"{"keep_extra_content":false}"#),
            ("file", Some("two.jar"), b"the second jar"),
        ]);

        let parts = collect(BOUNDARY, in_chunks(raw, 7), &dir.0, 1 << 20).await.expect("a read");
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0].file_name.as_deref(), Some("one.jar"));
        assert_eq!(parts[2].file_name.as_deref(), Some("two.jar"));

        match &parts[1].body {
            Content::Text(text) => assert_eq!(text, r#"{"keep_extra_content":false}"#),
            other => panic!("expected a field, got {other:?}"),
        }
        match &parts[0].body {
            Content::File { path, size } => {
                assert_eq!(std::fs::read(path).unwrap(), b"the first jar");
                assert_eq!(*size, 13);
            }
            other => panic!("expected a file, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn content_that_looks_like_a_boundary_survives_a_chunk_edge() {
        let dir = Dir::new();
        let awkward = format!("head\r\n--{BOUNDARY}xx tail\r\n--not-it\r\n").into_bytes();
        let raw = body(&[("file", Some("tricky.jar"), &awkward)]);

        for chunk in [1, 3, 8, 64, 4096] {
            let parts =
                collect(BOUNDARY, in_chunks(raw.clone(), chunk), &dir.0, 1 << 20).await.expect("a read");
            match &parts[0].body {
                Content::File { path, size } => {
                    assert_eq!(std::fs::read(path).unwrap(), awkward, "chunk size {chunk}");
                    assert_eq!(*size as usize, awkward.len());
                }
                other => panic!("expected a file, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn a_body_over_the_limit_is_refused_and_not_read_to_the_end() {
        let dir = Dir::new();
        let raw = body(&[("file", Some("big.jar"), &vec![b'x'; 4096])]);
        let refusal = collect(BOUNDARY, in_chunks(raw, 512), &dir.0, 1024).await.expect_err("too big");
        assert_eq!(refusal.code(), "file_too_large");
    }

    #[tokio::test]
    async fn a_body_that_stops_in_the_middle_is_an_error_and_not_a_short_file() {
        let dir = Dir::new();
        let mut raw = body(&[("file", Some("half.jar"), b"the first half")]);
        raw.truncate(raw.len() - 20);
        let refusal =
            collect(BOUNDARY, in_chunks(raw, 8), &dir.0, 1 << 20).await.expect_err("truncated");
        assert_eq!(refusal.code(), "invalid_request");
    }

    #[tokio::test]
    async fn an_empty_body_yields_nothing_rather_than_a_panic() {
        let dir = Dir::new();
        let refusal = collect(BOUNDARY, axum::body::Body::empty(), &dir.0, 1 << 20)
            .await
            .expect_err("nothing to read");
        assert_eq!(refusal.code(), "invalid_request");
    }
}
