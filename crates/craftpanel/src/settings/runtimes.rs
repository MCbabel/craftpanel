use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::model::JreVendor;

const WINDOW: Duration = Duration::from_secs(60);

const SEARCH: [&str; 4] = ["/usr/lib/jvm", "/usr/java", "/opt/java", "/opt/jdk"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JavaRuntime {
    pub major: u32,
    pub vendor: JreVendor,
    pub version: String,
    pub path: Option<String>,
    pub source: Source,
    pub installed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    System,
    Managed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JavaRuntimeList {
    pub runtimes: Vec<JavaRuntime>,
    pub default_major_for_game_version: Option<u32>,
}

pub fn discover(data_dir: &Path) -> Vec<JavaRuntime> {
    let mut found: BTreeMap<(u32, JreVendor), JavaRuntime> = BTreeMap::new();

    for (home, source) in candidates(data_dir) {
        let Some(runtime) = read_home(&home, source) else { continue };
        found.entry((runtime.major, runtime.vendor)).or_insert(runtime);
    }

    let mut runtimes: Vec<JavaRuntime> = found.into_values().collect();
    runtimes.sort_by(|left, right| {
        right.major.cmp(&left.major).then_with(|| left.vendor.as_str().cmp(right.vendor.as_str()))
    });
    runtimes
}

pub fn cached(data_dir: &Path) -> Vec<JavaRuntime> {
    static SEEN: OnceLock<Mutex<BTreeMap<PathBuf, (Instant, Vec<JavaRuntime>)>>> = OnceLock::new();
    let cell = SEEN.get_or_init(Mutex::default);

    let mut held = cell.lock().expect("the runtime cache outlives its panics");
    if let Some((at, runtimes)) = held.get(data_dir) {
        if at.elapsed() < WINDOW {
            return runtimes.clone();
        }
    }
    let runtimes = discover(data_dir);
    held.insert(data_dir.to_path_buf(), (Instant::now(), runtimes.clone()));
    runtimes
}

pub fn pick<'a>(
    runtimes: &'a [JavaRuntime],
    major: Option<u32>,
    vendor: Option<JreVendor>,
    game_version: Option<&str>,
) -> Option<&'a JavaRuntime> {
    let wanted = major.or_else(|| default_major(game_version?));
    let matches = |runtime: &&JavaRuntime| {
        wanted.is_none_or(|major| runtime.major == major)
            && vendor.is_none_or(|vendor| runtime.vendor == vendor)
    };

    runtimes
        .iter()
        .find(matches)
        .or_else(|| runtimes.iter().find(|runtime| vendor.is_none_or(|v| runtime.vendor == v)))
        .or_else(|| runtimes.first())
}

pub fn default_major(game_version: &str) -> Option<u32> {
    let mut parts = game_version.split('.');
    let major: u32 = parts.next()?.parse().ok()?;
    if major > 1 {
        return Some(if major >= 26 { 25 } else { 21 });
    }

    let minor: u32 = parts.next()?.split(['-', ' ']).next()?.parse().ok()?;
    Some(match minor {
        20.. => 21,
        17..=19 => 17,
        _ => 8,
    })
}

fn candidates(data_dir: &Path) -> Vec<(PathBuf, Source)> {
    let mut homes = Vec::new();

    if let Ok(entries) = std::fs::read_dir(data_dir.join("runtimes")) {
        for entry in entries.flatten() {
            homes.push((entry.path(), Source::Managed));
        }
    }
    if let Some(home) = std::env::var_os("JAVA_HOME") {
        homes.push((PathBuf::from(home), Source::System));
    }
    for root in SEARCH {
        if let Ok(entries) = std::fs::read_dir(root) {
            for entry in entries.flatten() {
                homes.push((entry.path(), Source::System));
            }
        }
    }
    if let Some(home) = on_path() {
        homes.push((home, Source::System));
    }
    homes
}

fn on_path() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join("java"))
        .find(|candidate| candidate.is_file())
        .and_then(|binary| std::fs::canonicalize(binary).ok())
        .and_then(|binary| Some(binary.parent()?.parent()?.to_path_buf()))
}

fn read_home(home: &Path, source: Source) -> Option<JavaRuntime> {
    if !home.join("bin").join("java").is_file() {
        return None;
    }
    let release = std::fs::read_to_string(home.join("release")).ok()?;
    let version = field(&release, "JAVA_VERSION")?;
    let major = major_of(&version)?;

    Some(JavaRuntime {
        major,
        vendor: vendor_of(field(&release, "IMPLEMENTOR").as_deref()),
        version,
        path: Some(home.join("bin").join("java").to_string_lossy().into_owned()),
        source,
        installed: true,
    })
}

fn field(release: &str, name: &str) -> Option<String> {
    release
        .lines()
        .find_map(|line| line.strip_prefix(name)?.strip_prefix('='))
        .map(|value| value.trim().trim_matches('"').to_owned())
}

fn major_of(version: &str) -> Option<u32> {
    let mut parts = version.split(['.', '_', '-', '+']);
    let first: u32 = parts.next()?.parse().ok()?;
    if first == 1 {
        parts.next()?.parse().ok()
    } else {
        Some(first)
    }
}

fn vendor_of(implementor: Option<&str>) -> JreVendor {
    let lowered = implementor.unwrap_or_default().to_ascii_lowercase();
    if lowered.contains("amazon") || lowered.contains("corretto") {
        JreVendor::Corretto
    } else if lowered.contains("graal") {
        JreVendor::Graal
    } else {
        JreVendor::Temurin
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::harness::a_dir;

    fn a_jdk(root: &Path, name: &str, release: &str) -> PathBuf {
        let home = root.join(name);
        std::fs::create_dir_all(home.join("bin")).unwrap();
        std::fs::write(home.join("bin").join("java"), "#!/bin/sh\n").unwrap();
        std::fs::write(home.join("release"), release).unwrap();
        home
    }

    #[test]
    fn a_release_file_answers_both_questions_without_running_anything() {
        let dir = a_dir();
        let home = a_jdk(
            dir.path(),
            "jdk-21",
            "IMPLEMENTOR=\"Eclipse Adoptium\"\nJAVA_VERSION=\"21.0.4\"\nOS_ARCH=\"x86_64\"\n",
        );

        let runtime = read_home(&home, Source::System).expect("a readable JDK");
        assert_eq!(runtime.major, 21);
        assert_eq!(runtime.vendor, JreVendor::Temurin);
        assert_eq!(runtime.version, "21.0.4");
        assert!(runtime.installed);
        assert!(runtime.path.unwrap().ends_with("jdk-21/bin/java"));
    }

    #[test]
    fn a_directory_without_a_binary_or_without_a_release_is_no_runtime() {
        let dir = a_dir();
        let bare = dir.path().join("empty");
        std::fs::create_dir_all(&bare).unwrap();
        assert_eq!(read_home(&bare, Source::System), None);

        let no_release = dir.path().join("no-release");
        std::fs::create_dir_all(no_release.join("bin")).unwrap();
        std::fs::write(no_release.join("bin").join("java"), "").unwrap();
        assert_eq!(read_home(&no_release, Source::System), None);
    }

    #[test]
    fn the_two_spellings_of_a_java_version_both_answer_a_major() {
        assert_eq!(major_of("1.8.0_422"), Some(8));
        assert_eq!(major_of("11.0.24"), Some(11));
        assert_eq!(major_of("21.0.4"), Some(21));
        assert_eq!(major_of("25-ea+15"), Some(25));
        assert_eq!(major_of("nonsense"), None);
    }

    #[test]
    fn the_three_vendors_are_read_out_of_the_implementor_and_the_rest_is_temurin() {
        assert_eq!(vendor_of(Some("Eclipse Adoptium")), JreVendor::Temurin);
        assert_eq!(vendor_of(Some("Amazon.com Inc.")), JreVendor::Corretto);
        assert_eq!(vendor_of(Some("Oracle GraalVM")), JreVendor::Graal);
        assert_eq!(vendor_of(Some("Debian")), JreVendor::Temurin);
        assert_eq!(vendor_of(None), JreVendor::Temurin);
    }

    #[test]
    fn a_managed_runtime_is_found_and_the_newest_major_comes_first() {
        let dir = a_dir();
        let runtimes = dir.path().join("runtimes");
        a_jdk(&runtimes, "temurin-21", "IMPLEMENTOR=\"Eclipse Adoptium\"\nJAVA_VERSION=\"21.0.4\"\n");
        a_jdk(&runtimes, "corretto-17", "IMPLEMENTOR=\"Amazon.com Inc.\"\nJAVA_VERSION=\"17.0.12\"\n");

        let found = discover(dir.path());
        let ours: Vec<&JavaRuntime> =
            found.iter().filter(|runtime| runtime.source == Source::Managed).collect();
        assert_eq!(ours.len(), 2, "{found:?}");
        assert_eq!(ours[0].major, 21, "newest first");
        assert_eq!(ours[1].vendor, JreVendor::Corretto);
    }

    #[test]
    fn the_default_major_follows_the_game_version_the_way_the_page_does() {
        assert_eq!(default_major("1.21.8"), Some(21));
        assert_eq!(default_major("1.20.1"), Some(21));
        assert_eq!(default_major("1.19.2"), Some(17));
        assert_eq!(default_major("1.17"), Some(17));
        assert_eq!(default_major("1.16.5"), Some(8));
        assert_eq!(default_major("1.8.9"), Some(8));
        assert_eq!(default_major("26.1"), Some(25));
        assert_eq!(default_major("3.5.1"), Some(21), "a Velocity line is not a game version");
        assert_eq!(default_major("24w14a"), None);
    }

    #[test]
    fn picking_prefers_the_asked_for_pair_and_falls_back_rather_than_refusing() {
        let runtimes = vec![
            JavaRuntime {
                major: 21,
                vendor: JreVendor::Temurin,
                version: "21.0.4".to_owned(),
                path: Some("/a/bin/java".to_owned()),
                source: Source::System,
                installed: true,
            },
            JavaRuntime {
                major: 8,
                vendor: JreVendor::Corretto,
                version: "1.8.0_422".to_owned(),
                path: Some("/b/bin/java".to_owned()),
                source: Source::System,
                installed: true,
            },
        ];

        assert_eq!(pick(&runtimes, Some(8), None, None).unwrap().major, 8);
        assert_eq!(pick(&runtimes, None, Some(JreVendor::Corretto), None).unwrap().major, 8);
        assert_eq!(pick(&runtimes, None, None, Some("1.16.5")).unwrap().major, 8);
        assert_eq!(pick(&runtimes, None, None, Some("1.21.8")).unwrap().major, 21);
        assert_eq!(pick(&runtimes, Some(11), None, None).unwrap().major, 21, "something over nothing");
        assert_eq!(pick(&[], Some(21), None, None), None);
    }
}
