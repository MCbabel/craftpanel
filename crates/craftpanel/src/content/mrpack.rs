use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::model::LoaderId;

use super::modrinth::MrHashes;
use super::paths::{self, PathFault};

const MAX_ENTRIES: usize = 20_000;
const MAX_UNPACKED: u64 = 2 * 1024 * 1024 * 1024;
const MAX_RATIO: u64 = 200;

const CDN: &str = "cdn.modrinth.com/data/";

#[derive(Debug, thiserror::Error)]
pub enum PackFault {
    #[error("this file is not a readable ZIP")]
    NotAnArchive,
    #[error("modrinth.index.json is missing or unreadable: {0}")]
    NoIndex(String),
    #[error("the pack names a path outside the server directory: {0}")]
    Escapes(String),
    #[error("the pack unpacks to more than we allow")]
    TooLarge,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl PackFault {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotAnArchive => "unsupported_archive",
            Self::NoIndex(_) | Self::Escapes(_) => "invalid_modpack",
            Self::TooLarge => "archive_too_large",
            Self::Io(_) => "internal",
        }
    }
}

pub type Result<T> = std::result::Result<T, PackFault>;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PackEnv {
    #[serde(default)]
    pub client: Option<String>,
    #[serde(default)]
    pub server: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PackFile {
    pub path: String,
    #[serde(default)]
    pub hashes: MrHashes,
    #[serde(default)]
    pub env: Option<PackEnv>,
    #[serde(default)]
    pub downloads: Vec<String>,
    #[serde(rename = "fileSize", default)]
    pub file_size: u64,
}

impl PackFile {
    pub fn wanted_on_a_server(&self) -> bool {
        self.env.as_ref().and_then(|env| env.server.as_deref()) != Some("unsupported")
    }

    pub fn modrinth_ids(&self) -> Option<(String, String)> {
        let url = self.downloads.iter().find(|url| url.contains(CDN))?;
        let rest = url.split_once(CDN)?.1;
        let mut parts = rest.split('/');
        let project = parts.next()?;
        if parts.next()? != "versions" {
            return None;
        }
        let version = parts.next()?;
        (!project.is_empty() && !version.is_empty())
            .then(|| (project.to_owned(), version.to_owned()))
    }

    pub fn external_url(&self) -> Option<&str> {
        self.modrinth_ids()
            .is_none()
            .then(|| self.downloads.first().map(String::as_str))
            .flatten()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Index {
    #[serde(rename = "formatVersion", default)]
    pub format_version: u32,
    #[serde(default)]
    pub game: String,
    #[serde(rename = "versionId", default)]
    pub version_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub files: Vec<PackFile>,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
}

impl Index {
    pub fn read(bytes: &[u8]) -> Result<Self> {
        let index: Self =
            serde_json::from_slice(bytes).map_err(|err| PackFault::NoIndex(err.to_string()))?;
        if index.game != "minecraft" && !index.game.is_empty() {
            return Err(PackFault::NoIndex(format!("{} is not Minecraft", index.game)));
        }
        Ok(index)
    }

    pub fn game_version(&self) -> Option<&str> {
        self.dependencies.get("minecraft").map(String::as_str)
    }

    pub fn loader(&self) -> Option<(LoaderId, String)> {
        let known = [
            ("fabric-loader", LoaderId::Fabric),
            ("quilt-loader", LoaderId::Quilt),
            ("neoforge", LoaderId::NeoForge),
            ("forge", LoaderId::Forge),
        ];
        known.into_iter().find_map(|(key, loader)| {
            self.dependencies.get(key).map(|version| (loader, version.clone()))
        })
    }

    pub fn server_files(&self) -> Result<Vec<(String, &PackFile)>> {
        let mut out = Vec::new();
        for file in &self.files {
            if file.path.starts_with('/') {
                return Err(escapes(&file.path));
            }
            let path = paths::relative(&file.path).map_err(|_| escapes(&file.path))?;
            if path.is_empty() {
                return Err(escapes(&file.path));
            }
            if file.wanted_on_a_server() {
                out.push((path, file));
            }
        }
        Ok(out)
    }

    pub fn omitted_projects(&self) -> BTreeSet<String> {
        self.files
            .iter()
            .filter(|file| !file.wanted_on_a_server())
            .filter_map(|file| file.modrinth_ids().map(|(project, _)| project))
            .collect()
    }
}

fn escapes(path: &str) -> PackFault {
    PackFault::Escapes(path.to_owned())
}

const OVERRIDES: [&str; 2] = ["overrides/", "server-overrides/"];

#[derive(Debug)]
pub struct Pack {
    archive: zip::ZipArchive<std::fs::File>,
    packed_bytes: u64,
    pub index: Index,
}

impl Pack {
    pub fn open(path: &Path) -> Result<Self> {
        let file = std::fs::File::open(path)?;
        let packed_bytes = file.metadata()?.len();
        let mut archive = zip::ZipArchive::new(file).map_err(|_| PackFault::NotAnArchive)?;
        if archive.len() > MAX_ENTRIES {
            return Err(PackFault::TooLarge);
        }

        let mut bytes = Vec::new();
        archive
            .by_name("modrinth.index.json")
            .map_err(|err| PackFault::NoIndex(err.to_string()))?
            .read_to_end(&mut bytes)?;

        Ok(Self { index: Index::read(&bytes)?, archive, packed_bytes })
    }

    pub fn extract_overrides(&mut self, into: &Path) -> Result<Vec<String>> {
        let mut written = Vec::new();
        let mut unpacked = 0u64;

        for prefix in OVERRIDES {
            for position in 0..self.archive.len() {
                let mut entry = self.archive.by_index(position).map_err(|_| PackFault::NotAnArchive)?;
                let Some(name) = entry.enclosed_name().map(PathBuf::from) else {
                    return Err(escapes(&entry.name().to_owned()));
                };
                let Some(name) = name.to_str() else { continue };
                let Some(rest) = name.strip_prefix(prefix) else { continue };
                if rest.is_empty() || entry.is_dir() {
                    continue;
                }

                let relative = paths::relative(rest).map_err(|_| escapes(name))?;
                if relative.is_empty() {
                    continue;
                }
                let target = paths::resolve_leaf(into, &relative)
                    .map_err(|fault| lay_out_fault(fault, name))?;

                unpacked += entry.size();
                if unpacked > MAX_UNPACKED
                    || unpacked > self.packed_bytes.saturating_mul(MAX_RATIO).max(MAX_RATIO)
                {
                    return Err(PackFault::TooLarge);
                }

                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                paths::clear_destination(&target)?;
                let mut out = std::fs::File::create(&target)?;
                std::io::copy(&mut entry, &mut out)?;

                if !written.contains(&relative) {
                    written.push(relative);
                }
            }
        }

        Ok(written)
    }
}

fn lay_out_fault(fault: PathFault, name: &str) -> PackFault {
    match fault {
        PathFault::TooLong => PackFault::TooLarge,
        _ => escapes(name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn deflated(entries: &[(&str, &[u8])]) -> PathBuf {
        write_zip(entries, zip::CompressionMethod::Deflated)
    }

    fn zipped(entries: &[(&str, &[u8])]) -> PathBuf {
        write_zip(entries, zip::CompressionMethod::Stored)
    }

    fn write_zip(entries: &[(&str, &[u8])], method: zip::CompressionMethod) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("craftpanel-pack-{}.mrpack", crate::model::Id::new()));
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&path).expect("a file"));
        let options: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(method);
        for (name, body) in entries {
            writer.start_file(*name, options).expect("an entry");
            writer.write_all(body).expect("the body");
        }
        writer.finish().expect("a finished archive");
        path
    }

    fn index_json(files: serde_json::Value) -> Vec<u8> {
        serde_json::json!({
            "formatVersion": 1,
            "game": "minecraft",
            "versionId": "1.1",
            "name": "Create Lite 1.1",
            "files": files,
            "dependencies": { "minecraft": "1.19.2", "quilt-loader": "0.19.0-beta.18" }
        })
        .to_string()
        .into_bytes()
    }

    #[test]
    fn the_index_of_a_real_pack_reads_as_the_format_describes_it() {
        let index = Index::read(&index_json(serde_json::json!([{
            "path": "mods/alternate-current-mc1.19-1.6.0.jar",
            "downloads": ["https://cdn.modrinth.com/data/r0v8vy1s/versions/4nKkKsjy/alternate-current-mc1.19-1.6.0.jar"],
            "hashes": {
                "sha512": "43c93cfb44a5b6b598fc0420fe980323393d0389955913fa17b3639e67203d758d8d1522c9df5d929db5ee19a30187f2f33d581ee36a1ac4dce9064ae91ad714",
                "sha1": "2b45c907353efdfd4feada481f76be2ea920d7fc"
            },
            "fileSize": 38349
        }])))
        .expect("a readable index");

        assert_eq!(index.game_version(), Some("1.19.2"));
        assert_eq!(index.loader(), Some((LoaderId::Quilt, "0.19.0-beta.18".to_owned())));
        let files = index.server_files().expect("paths inside the server");
        assert_eq!(files[0].0, "mods/alternate-current-mc1.19-1.6.0.jar");
        assert_eq!(
            files[0].1.modrinth_ids(),
            Some(("r0v8vy1s".to_owned(), "4nKkKsjy".to_owned()))
        );
        assert!(files[0].1.external_url().is_none());
    }

    #[test]
    fn a_client_only_file_is_not_laid_out_and_counts_as_omitted() {
        let index = Index::read(&index_json(serde_json::json!([
            {
                "path": "mods/sodium.jar",
                "downloads": ["https://cdn.modrinth.com/data/AANobbMI/versions/aaa/sodium.jar"],
                "env": { "client": "required", "server": "unsupported" }
            },
            {
                "path": "mods/lithium.jar",
                "downloads": ["https://cdn.modrinth.com/data/gvQqBUqZ/versions/bbb/lithium.jar"],
                "env": { "client": "required", "server": "required" }
            }
        ])))
        .expect("a readable index");

        let laid_out = index.server_files().expect("paths inside the server");
        assert_eq!(laid_out.len(), 1);
        assert_eq!(laid_out[0].0, "mods/lithium.jar");
        assert_eq!(index.omitted_projects(), BTreeSet::from(["AANobbMI".to_owned()]));
    }

    #[test]
    fn a_file_from_somewhere_else_keeps_its_address() {
        let index = Index::read(&index_json(serde_json::json!([{
            "path": "mods/private.jar",
            "downloads": ["https://example.invalid/private.jar"],
            "hashes": { "sha512": "ab" }
        }])))
        .expect("a readable index");
        let file = &index.files[0];
        assert_eq!(file.external_url(), Some("https://example.invalid/private.jar"));
        assert!(file.modrinth_ids().is_none());
    }

    #[test]
    fn an_index_path_that_climbs_out_is_refused_and_the_whole_pack_with_it() {
        for path in ["../../etc/cron.d/mine", "/etc/passwd", "mods/../../escape.jar"] {
            let index = Index::read(&index_json(serde_json::json!([{
                "path": path,
                "downloads": ["https://cdn.modrinth.com/data/A/versions/B/x.jar"]
            }])))
            .expect("the index itself parses");
            let refusal = index.server_files().expect_err(&format!("{path} must be refused"));
            assert_eq!(refusal.code(), "invalid_modpack", "{path}");
        }
    }

    #[test]
    fn an_override_that_climbs_out_writes_nothing() {
        let archive = zipped(&[
            ("modrinth.index.json", &index_json(serde_json::json!([]))),
            ("overrides/../../escaped.txt", b"nope"),
        ]);
        let into = std::env::temp_dir().join(format!("craftpanel-into-{}", crate::model::Id::new()));
        std::fs::create_dir_all(&into).expect("a work directory");

        let mut pack = Pack::open(&archive).expect("a pack");
        let refusal = pack.extract_overrides(&into).expect_err("the entry climbs out");
        assert_eq!(refusal.code(), "invalid_modpack");
        assert!(!into.parent().expect("a parent").join("escaped.txt").exists());

        let _ = std::fs::remove_file(&archive);
        let _ = std::fs::remove_dir_all(&into);
    }

    #[test]
    fn server_overrides_win_over_overrides_and_client_overrides_are_ignored() {
        let archive = zipped(&[
            ("modrinth.index.json", &index_json(serde_json::json!([]))),
            ("overrides/config/a.toml", b"from overrides"),
            ("overrides/config/only-here.toml", b"kept"),
            ("server-overrides/config/a.toml", b"from server-overrides"),
            ("client-overrides/options.txt", b"not ours"),
        ]);
        let into = std::env::temp_dir().join(format!("craftpanel-into-{}", crate::model::Id::new()));
        std::fs::create_dir_all(&into).expect("a work directory");

        let mut pack = Pack::open(&archive).expect("a pack");
        let written = pack.extract_overrides(&into).expect("the overrides land");

        assert_eq!(
            std::fs::read(into.join("config").join("a.toml")).expect("the file"),
            b"from server-overrides"
        );
        assert!(into.join("config").join("only-here.toml").exists());
        assert!(!into.join("options.txt").exists(), "client overrides are dropped (8.17)");
        assert_eq!(written.len(), 2);

        let _ = std::fs::remove_file(&archive);
        let _ = std::fs::remove_dir_all(&into);
    }

    #[test]
    fn a_file_that_is_no_zip_is_refused_before_anything_else() {
        let path = std::env::temp_dir().join(format!("craftpanel-{}.mrpack", crate::model::Id::new()));
        std::fs::write(&path, b"this is not a zip").expect("a file");
        let refusal = Pack::open(&path).expect_err("not an archive");
        assert_eq!(refusal.code(), "unsupported_archive");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_zip_without_an_index_is_not_a_modpack() {
        let archive = zipped(&[("mods/foo.jar", b"jar")]);
        let refusal = Pack::open(&archive).expect_err("no index");
        assert_eq!(refusal.code(), "invalid_modpack");
        let _ = std::fs::remove_file(&archive);
    }

    #[test]
    fn an_archive_that_unpacks_out_of_all_proportion_is_refused() {
        let mut entries: Vec<(String, Vec<u8>)> =
            vec![("modrinth.index.json".to_owned(), index_json(serde_json::json!([])))];
        for index in 0..40 {
            entries.push((format!("overrides/blob{index}.bin"), vec![0u8; 1024 * 1024]));
        }
        let borrowed: Vec<(&str, &[u8])> =
            entries.iter().map(|(name, body)| (name.as_str(), body.as_slice())).collect();

        let bomb = deflated(&borrowed);
        let into = std::env::temp_dir().join(format!("craftpanel-into-{}", crate::model::Id::new()));
        std::fs::create_dir_all(&into).expect("a work directory");

        let mut pack = Pack::open(&bomb).expect("a readable archive");
        let refusal = pack.extract_overrides(&into).expect_err("that is not a modpack");
        assert_eq!(refusal.code(), "archive_too_large");

        let honest = zipped(&borrowed);
        let mut pack = Pack::open(&honest).expect("a readable archive");
        assert!(pack.extract_overrides(&into).is_ok(), "40 MiB stored is not a bomb");

        let _ = std::fs::remove_file(&bomb);
        let _ = std::fs::remove_file(&honest);
        let _ = std::fs::remove_dir_all(&into);
    }

    #[tokio::test]
    #[ignore = "downloads a modpack from Modrinth"]
    async fn live_a_real_mrpack_reads_the_way_8_17_describes() {
        let url = "https://cdn.modrinth.com/data/WZwys3LN/versions/BJSvLVwK/Create%20Lite%201.1.mrpack";
        let bytes = reqwest::Client::builder()
            .user_agent(super::super::modrinth::AGENT)
            .build()
            .expect("a client")
            .get(url)
            .send()
            .await
            .expect("Modrinth answers")
            .bytes()
            .await
            .expect("the pack");

        let path = std::env::temp_dir().join(format!("craftpanel-live-{}.mrpack", crate::model::Id::new()));
        std::fs::write(&path, &bytes).expect("the pack on disk");

        let pack = Pack::open(&path).expect("a readable pack");
        assert_eq!(pack.index.format_version, 1);
        assert_eq!(pack.index.game_version(), Some("1.19.2"));
        assert!(matches!(pack.index.loader(), Some((LoaderId::Quilt, _))));

        let files = pack.index.server_files().expect("paths inside the server");
        assert_eq!(files.len(), 42);
        assert!(files.iter().all(|(path, _)| path.starts_with("mods/")));
        assert!(files.iter().all(|(_, file)| file.hashes.sha512.is_some()));
        assert!(files.iter().all(|(_, file)| file.modrinth_ids().is_some()));

        let _ = std::fs::remove_file(&path);
    }
}
