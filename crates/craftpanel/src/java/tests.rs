#![cfg(test)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::model::JreVendor;
use crate::settings::runtimes::{self, Search, Source};

use super::harness::{self, a_data_dir, FakeAdoptium, Scratch};
use super::{JavaError, Runtimes, Stage};

const VERSION: &str = "21.0.12+7";
const A_TEMURIN_IS_DEEP: usize = 6;

fn root() -> String {
    format!("jdk-{VERSION}-jre")
}

fn here() -> &'static str {
    super::Arch::here().expect("a machine we serve").as_str()
}

fn version_of(binary: &Path) -> String {
    let shown = std::process::Command::new(binary)
        .arg("-version")
        .output()
        .expect("the unpacked java runs");
    String::from_utf8_lossy(&shown.stderr).into_owned()
}

async fn refused(bytes: Vec<u8>) -> (JavaError, Scratch) {
    let dir = a_data_dir();
    let upstream = FakeAdoptium::started().await;
    upstream.offer(21, VERSION, bytes);

    let runtimes = Runtimes::with_base(dir.path(), upstream.base()).expect("a client");
    let refusal = runtimes.install(21).await.expect_err("it must refuse");

    assert!(!dir.path().join("runtimes").join("java-21").exists(), "nothing was laid down");
    assert!(
        !dir.path().join("runtimes").join(".java-21.new").exists(),
        "and the half-unpacked tree was swept up"
    );
    (refusal, dir)
}

#[tokio::test]
async fn a_missing_runtime_is_fetched_and_lands_where_the_resolver_looks_first() {
    let dir = a_data_dir();
    let upstream = FakeAdoptium::started().await;
    let archive = harness::a_jre(VERSION);
    upstream.offer(21, VERSION, archive.clone());

    let runtimes = Runtimes::with_base(dir.path(), upstream.base()).expect("a client");
    let progress = runtimes.watch(21);
    let installed = runtimes.install(21).await.expect("a runtime");

    let home = dir.path().join("runtimes").join("java-21");
    let binary = home.join("bin").join("java");
    assert_eq!(installed.home, home, "exactly where Manager::java looks first");
    assert!(installed.fresh);
    assert!(binary.is_file(), "the root directory of the archive was stripped");
    assert!(
        std::fs::metadata(&binary).expect("the launcher").permissions().mode() & 0o111 != 0,
        "and it may be run"
    );
    assert!(version_of(&binary).contains(VERSION));

    assert_eq!(installed.runtime.source, Source::Managed);
    assert_eq!(installed.runtime.major, 21);
    assert_eq!(installed.runtime.version, VERSION);
    assert_eq!(installed.runtime.vendor, JreVendor::Temurin);
    assert_eq!(installed.runtime.path.as_deref(), binary.to_str());

    let seen = runtimes::discover(dir.path(), &Search::nowhere());
    assert!(
        seen.iter().any(|found| found.source == Source::Managed && found.major == 21),
        "settings::runtimes finds it too: {seen:?}"
    );

    assert_eq!(progress.stage(), Stage::Done);
    assert_eq!(progress.total(), archive.len() as u64);
    assert_eq!(progress.share(), 1.0);
    assert_eq!(upstream.served(), 1);
    assert!(!dir.path().join("runtimes").join(".java-21.new").exists());
}

#[tokio::test]
async fn we_ask_adoptium_for_a_headless_linux_jre_of_this_machine() {
    let dir = a_data_dir();
    let upstream = FakeAdoptium::started().await;
    upstream.offer(21, VERSION, harness::a_jre(VERSION));

    let runtimes = Runtimes::with_base(dir.path(), upstream.base()).expect("a client");
    runtimes.install(21).await.expect("a runtime");

    let asked = upstream.queries();
    assert_eq!(asked.len(), 1);
    assert_eq!(
        asked[0],
        format!("architecture={}&image_type=jre&os=linux&vendor=eclipse", here())
    );
}

#[tokio::test]
async fn a_runtime_that_is_already_there_is_not_fetched_a_second_time() {
    let dir = a_data_dir();
    let upstream = FakeAdoptium::started().await;
    upstream.offer(21, VERSION, harness::a_jre(VERSION));

    let runtimes = Runtimes::with_base(dir.path(), upstream.base()).expect("a client");
    assert!(runtimes.present(21).is_none());
    assert!(runtimes.install(21).await.expect("a runtime").fresh);

    let again = runtimes.install(21).await.expect("the one that is there");
    assert!(!again.fresh);
    assert_eq!(upstream.served(), 1, "the archive was fetched once");
    assert_eq!(upstream.asked(), 1, "and Adoptium was not even asked again");
    assert_eq!(runtimes.present(21).map(|found| found.major), Some(21));
}

#[tokio::test]
async fn two_callers_who_need_the_same_java_wait_on_one_download() {
    let dir = a_data_dir();
    let upstream = FakeAdoptium::started().await;
    upstream.offer(21, VERSION, harness::a_jre(VERSION));
    upstream.hold(Duration::from_millis(200));

    let runtimes = Arc::new(Runtimes::with_base(dir.path(), upstream.base()).expect("a client"));
    let first = {
        let runtimes = Arc::clone(&runtimes);
        tokio::spawn(async move { runtimes.install(21).await })
    };
    let second = {
        let runtimes = Arc::clone(&runtimes);
        tokio::spawn(async move { runtimes.install(21).await })
    };

    let (first, second) = tokio::try_join!(first, second).expect("both tasks");
    let (first, second) = (first.expect("a runtime"), second.expect("a runtime"));

    assert_eq!(upstream.served(), 1, "only one download for both of them");
    assert_eq!(upstream.asked(), 1);
    assert_ne!(first.fresh, second.fresh, "one laid it down, the other found it");
    assert_eq!(first.home, second.home);
}

#[tokio::test]
async fn bytes_that_do_not_match_the_announced_checksum_install_nothing() {
    let dir = a_data_dir();
    let upstream = FakeAdoptium::started().await;
    upstream.offer_announcing(21, VERSION, harness::a_jre(VERSION), &"0".repeat(64));

    let runtimes = Runtimes::with_base(dir.path(), upstream.base()).expect("a client");
    let refusal = runtimes.install(21).await.expect_err("a mismatch must fail");

    assert_eq!(refusal.code(), "java_download_damaged");
    assert!(refusal.to_string().contains("is damaged"), "{refusal}");
    assert!(!dir.path().join("runtimes").join("java-21").exists());
    assert!(!dir.path().join("runtimes").join(".java-21.new").exists());
    assert!(runtimes.present(21).is_none());
}

#[tokio::test]
async fn an_answer_that_names_no_checksum_is_not_downloaded_at_all() {
    let dir = a_data_dir();
    let upstream = FakeAdoptium::started().await;
    upstream.offer_unchecked(21, VERSION, harness::a_jre(VERSION));

    let runtimes = Runtimes::with_base(dir.path(), upstream.base()).expect("a client");
    let refusal = runtimes.install(21).await.expect_err("no checksum, no install");

    assert_eq!(refusal.code(), "java_download_unavailable");
    assert_eq!(upstream.served(), 0);
}

#[tokio::test]
async fn a_major_adoptium_does_not_build_says_so() {
    let dir = a_data_dir();
    let upstream = FakeAdoptium::started().await;

    let runtimes = Runtimes::with_base(dir.path(), upstream.base()).expect("a client");
    let refusal = runtimes.install(26).await.expect_err("nothing is offered");

    assert_eq!(
        refusal.to_string(),
        format!("Adoptium has no Java 26 runtime for linux/{}", here())
    );
    assert!(!dir.path().join("runtimes").join("java-26").exists());
}

#[test]
fn a_machine_nobody_builds_java_for_is_told_what_to_do_instead() {
    let refusal = JavaError::UnsupportedMachine { arch: "riscv64".to_owned(), major: 21 };

    assert_eq!(refusal.code(), "java_download_unsupported");
    assert_eq!(
        refusal.to_string(),
        "no Java can be downloaded for this machine: Adoptium builds Linux runtimes for \
         x64 and aarch64, and this one is riscv64. Install Java 21 with the package \
         manager instead (apt install openjdk-21-jre-headless)."
    );
}

#[tokio::test]
async fn a_place_we_cannot_write_is_named_before_anything_is_fetched() {
    let dir = a_data_dir();
    std::fs::write(dir.path().join("runtimes"), b"not a directory").expect("a file in the way");

    let upstream = FakeAdoptium::started().await;
    upstream.offer(21, VERSION, harness::a_jre(VERSION));
    let runtimes = Runtimes::with_base(dir.path(), upstream.base()).expect("a client");
    let refusal = runtimes.install(21).await.expect_err("there is nowhere to put it");

    assert_eq!(refusal.code(), "java_runtime_unwritable");
    assert!(refusal.to_string().contains(".java-21.new"), "{refusal}");
    assert_eq!(upstream.asked(), 0, "and Adoptium was not asked for nothing");
}

#[tokio::test]
async fn an_entry_that_climbs_out_of_the_archive_takes_the_whole_run_with_it() {
    let archive = harness::tarball(|builder| {
        harness::file(builder, &format!("{}/bin/java", root()), b"#!/bin/sh\n", 0o755);
        harness::raw_named(builder, b"../escaped.txt", b"owned");
    });

    let (refusal, dir) = refused(archive).await;
    assert_eq!(refusal.code(), "java_archive_rejected");
    assert!(refusal.to_string().contains("leaves the directory"), "{refusal}");
    assert!(!dir.path().join("runtimes").join("escaped.txt").exists());
}

#[tokio::test]
async fn an_entry_with_an_absolute_path_writes_nowhere() {
    let dir = a_data_dir();
    let planted = dir.path().join("planted.txt");
    let archive = harness::tarball(|builder| {
        harness::file(builder, &format!("{}/bin/java", root()), b"#!/bin/sh\n", 0o755);
        harness::raw_named(builder, planted.to_str().expect("a name").as_bytes(), b"owned");
    });

    let upstream = FakeAdoptium::started().await;
    upstream.offer(21, VERSION, archive);
    let runtimes = Runtimes::with_base(dir.path(), upstream.base()).expect("a client");
    let refusal = runtimes.install(21).await.expect_err("it must refuse");

    assert_eq!(refusal.code(), "java_archive_rejected");
    assert!(!planted.exists(), "nothing was written where the archive pointed");
    assert!(!dir.path().join("runtimes").join("java-21").exists());
}

#[tokio::test]
async fn a_link_that_points_out_of_the_runtime_is_refused() {
    let archive = harness::tarball(|builder| {
        harness::file(builder, &format!("{}/bin/java", root()), b"#!/bin/sh\n", 0o755);
        harness::link(builder, &format!("{}/keys", root()), "/etc/shadow");
    });

    let (refusal, _dir) = refused(archive).await;
    assert_eq!(refusal.code(), "java_archive_rejected");
    assert!(refusal.to_string().contains("/etc/shadow"), "{refusal}");
}

#[tokio::test]
async fn a_chain_of_links_cannot_walk_out_either() {
    let archive = harness::tarball(|builder| {
        harness::file(builder, &format!("{}/bin/java", root()), b"#!/bin/sh\n", 0o755);
        harness::link(builder, &format!("{}/lib/door", root()), "..");
        harness::link(builder, &format!("{}/lib/out", root()), "door/../../elsewhere");
    });

    let (refusal, dir) = refused(archive).await;
    assert_eq!(refusal.code(), "java_archive_rejected");
    assert!(refusal.to_string().contains("a link out of the runtime"), "{refusal}");
    assert!(!dir.path().join("runtimes").join("elsewhere").exists());
}

#[tokio::test]
async fn a_link_that_stays_inside_is_laid_down_the_way_temurin_ships_it() {
    let dir = a_data_dir();
    let upstream = FakeAdoptium::started().await;
    upstream.offer(21, VERSION, harness::a_jre(VERSION));

    let runtimes = Runtimes::with_base(dir.path(), upstream.base()).expect("a client");
    let installed = runtimes.install(21).await.expect("a runtime");

    let link = installed.home.join("lib").join("server").join("libjsig.so");
    assert!(link.symlink_metadata().expect("the link").file_type().is_symlink());
    assert_eq!(std::fs::read(&link).expect("through the link"), b"not really a library");
}

#[tokio::test]
async fn a_hard_link_in_the_archive_is_refused() {
    let archive = harness::tarball(|builder| {
        harness::file(builder, &format!("{}/bin/java", root()), b"#!/bin/sh\n", 0o755);
        harness::hard_link(builder, &format!("{}/keys", root()), "/etc/passwd");
    });

    let (refusal, _dir) = refused(archive).await;
    assert_eq!(refusal.code(), "java_archive_rejected");
    assert!(refusal.to_string().contains("hard link"), "{refusal}");
}

#[tokio::test]
async fn an_archive_with_two_root_directories_is_not_a_runtime() {
    let archive = harness::tarball(|builder| {
        harness::file(builder, &format!("{}/bin/java", root()), b"#!/bin/sh\n", 0o755);
        harness::file(builder, "somewhere-else/bin/java", b"#!/bin/sh\n", 0o755);
    });

    let (refusal, _dir) = refused(archive).await;
    assert_eq!(refusal.code(), "java_archive_rejected");
    assert!(refusal.to_string().contains("two root directories"), "{refusal}");
}

#[tokio::test]
async fn an_archive_without_a_launcher_is_not_laid_down() {
    let archive = harness::tarball(|builder| {
        harness::file(builder, &format!("{}/release", root()), b"JAVA_VERSION=\"21\"\n", 0o644);
    });

    let (refusal, _dir) = refused(archive).await;
    assert_eq!(refusal.code(), "java_runtime_incomplete");
    assert!(refusal.to_string().contains("bin/java is not there"), "{refusal}");
}

#[tokio::test]
async fn a_launcher_that_cannot_be_run_is_not_laid_down() {
    let archive = harness::tarball(|builder| {
        harness::file(builder, &format!("{}/bin/java", root()), b"#!/bin/sh\n", 0o644);
    });

    let (refusal, _dir) = refused(archive).await;
    assert_eq!(refusal.code(), "java_runtime_incomplete");
    assert!(refusal.to_string().contains("bin/java cannot be run"), "{refusal}");
}

#[tokio::test]
async fn an_unpacked_tree_without_a_release_file_is_no_runtime_either() {
    let archive = harness::tarball(|builder| {
        harness::file(builder, &format!("{}/bin/java", root()), b"#!/bin/sh\n", 0o755);
    });

    let (refusal, dir) = refused(archive).await;
    assert_eq!(refusal.code(), "java_runtime_incomplete");
    assert!(refusal.to_string().contains("no readable release file"), "{refusal}");
    let seen = runtimes::discover(dir.path(), &Search::nowhere());
    assert!(seen.iter().all(|found| found.source != Source::Managed));
}

#[tokio::test]
async fn an_archive_that_holds_another_major_than_the_one_asked_for_is_refused() {
    let archive = harness::tarball(|builder| {
        harness::file(builder, &format!("{}/bin/java", root()), b"#!/bin/sh\n", 0o755);
        harness::file(
            builder,
            &format!("{}/release", root()),
            b"IMPLEMENTOR=\"Eclipse Adoptium\"\nJAVA_VERSION=\"17.0.20\"\n",
            0o644,
        );
    });

    let (refusal, _dir) = refused(archive).await;
    assert_eq!(refusal.code(), "java_runtime_incomplete");
    assert!(refusal.to_string().contains("it is Java 17, not the Java 21"), "{refusal}");
}

#[tokio::test]
async fn what_an_earlier_failure_left_behind_is_swept_up_before_the_next_try() {
    let dir = a_data_dir();
    let leftover = dir.path().join("runtimes").join(".java-21.new");
    std::fs::create_dir_all(leftover.join("tree").join("bin")).expect("the leftovers");
    std::fs::write(leftover.join("tree").join("bin").join("java"), b"half a download")
        .expect("a leftover launcher");

    let upstream = FakeAdoptium::started().await;
    upstream.offer(21, VERSION, harness::a_jre(VERSION));
    let runtimes = Runtimes::with_base(dir.path(), upstream.base()).expect("a client");
    let installed = runtimes.install(21).await.expect("a runtime");

    assert!(!leftover.exists());
    assert!(version_of(&installed.home.join("bin").join("java")).contains(VERSION));
}

#[tokio::test]
async fn a_half_written_runtime_from_before_is_replaced_whole() {
    let dir = a_data_dir();
    let home = dir.path().join("runtimes").join("java-21");
    std::fs::create_dir_all(home.join("bin")).expect("the ruins");
    std::fs::write(home.join("nonsense.txt"), b"left over").expect("a stray file");

    let upstream = FakeAdoptium::started().await;
    upstream.offer(21, VERSION, harness::a_jre(VERSION));
    let runtimes = Runtimes::with_base(dir.path(), upstream.base()).expect("a client");
    let installed = runtimes.install(21).await.expect("a runtime");

    assert!(installed.fresh);
    assert!(!home.join("nonsense.txt").exists(), "the ruins went with the swap");
    assert!(home.join("release").is_file());
    assert!(version_of(&home.join("bin").join("java")).contains(VERSION));
}

#[tokio::test]
async fn the_size_and_the_stand_are_readable_while_the_download_runs() {
    let dir = a_data_dir();
    let upstream = FakeAdoptium::started().await;
    let archive = harness::a_jre(VERSION);
    upstream.offer(21, VERSION, archive.clone());
    upstream.shut();

    let runtimes = Arc::new(Runtimes::with_base(dir.path(), upstream.base()).expect("a client"));
    let progress = runtimes.watch(21);
    let task = {
        let runtimes = Arc::clone(&runtimes);
        tokio::spawn(async move { runtimes.install(21).await })
    };

    for _ in 0..500 {
        if progress.stage() == Stage::Downloading {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(progress.stage(), Stage::Downloading);
    assert_eq!(progress.total(), archive.len() as u64, "the size is out before the bytes are");
    assert!(progress.share() < 0.9);

    upstream.open();
    task.await.expect("the task").expect("a runtime");
    assert_eq!(progress.stage(), Stage::Done);
    assert_eq!(progress.share(), 1.0);
}

#[tokio::test]
async fn a_link_that_leads_off_adoptiums_own_hosts_is_never_even_asked_for() {
    let dir = a_data_dir();
    let upstream = FakeAdoptium::started().await;
    let elsewhere = FakeAdoptium::started().await;
    upstream.offer(21, VERSION, harness::a_jre(VERSION));
    elsewhere.offer(21, VERSION, harness::a_jre(VERSION));
    upstream.point_at(&elsewhere.binary_url(VERSION));

    let runtimes = Runtimes::with_base(dir.path(), upstream.base()).expect("a client");
    let refusal = runtimes.install(21).await.expect_err("a strange host must be refused");

    assert_eq!(refusal.code(), "java_download_untrusted");
    assert!(refusal.to_string().contains(elsewhere.base()), "{refusal}");
    assert_eq!(elsewhere.served(), 0, "the other machine was never asked for a byte");
    assert!(!dir.path().join("runtimes").join("java-21").exists());
    assert!(!dir.path().join("runtimes").join(".java-21.new").exists());
    assert!(runtimes.present(21).is_none());
}

#[tokio::test]
async fn a_redirect_onto_another_host_is_broken_off_instead_of_followed() {
    let dir = a_data_dir();
    let upstream = FakeAdoptium::started().await;
    let elsewhere = FakeAdoptium::started().await;
    upstream.offer(21, VERSION, harness::a_jre(VERSION));
    elsewhere.offer(21, VERSION, harness::a_jre(VERSION));
    upstream.point_at(&upstream.detour_url(VERSION));
    upstream.detour_to(&elsewhere.binary_url(VERSION));

    let runtimes = Runtimes::with_base(dir.path(), upstream.base()).expect("a client");
    let refusal = runtimes.install(21).await.expect_err("the detour must be refused");

    assert_eq!(refusal.code(), "java_download_untrusted");
    assert!(refusal.to_string().contains(elsewhere.base()), "{refusal}");
    assert_eq!(elsewhere.served(), 0, "the other machine was never asked for a byte");
    assert!(!dir.path().join("runtimes").join("java-21").exists());
    assert!(!dir.path().join("runtimes").join(".java-21.new").exists());
}

#[tokio::test]
async fn a_redirect_that_stays_on_a_host_we_trust_is_followed() {
    let dir = a_data_dir();
    let upstream = FakeAdoptium::started().await;
    upstream.offer(21, VERSION, harness::a_jre(VERSION));
    upstream.point_at(&upstream.detour_url(VERSION));
    upstream.detour_to(&upstream.binary_url(VERSION));

    let runtimes = Runtimes::with_base(dir.path(), upstream.base()).expect("a client");
    let installed = runtimes.install(21).await.expect("a runtime");

    assert!(installed.fresh);
    assert_eq!(upstream.served(), 1);
    assert!(version_of(&installed.home.join("bin").join("java")).contains(VERSION));
}

#[tokio::test]
async fn a_download_that_runs_past_the_size_it_announced_is_cut_off_where_it_bursts() {
    let dir = a_data_dir();
    let upstream = FakeAdoptium::started().await;
    upstream.offer_sized(21, VERSION, vec![0u8; 16 * 1024 * 1024], 1024);

    let runtimes = Runtimes::with_base(dir.path(), upstream.base()).expect("a client");
    let progress = runtimes.watch(21);
    let refusal = runtimes.install(21).await.expect_err("the ceiling must hold");

    assert_eq!(refusal.code(), "java_download_oversized");
    assert!(refusal.to_string().contains("longer than"), "{refusal}");
    assert_eq!(progress.total(), 1024, "the size Adoptium announced");
    assert!(
        progress.done() < 4 * 1024 * 1024,
        "{} of the 16 MiB were pulled off the wire",
        progress.done()
    );
    assert!(!dir.path().join("runtimes").join(".java-21.new").exists());
    assert!(!dir.path().join("runtimes").join("java-21").exists());
}

#[tokio::test]
async fn a_size_no_runtime_could_have_is_refused_before_a_byte_is_asked_for() {
    let dir = a_data_dir();
    let upstream = FakeAdoptium::started().await;
    upstream.offer_sized(21, VERSION, harness::a_jre(VERSION), 8 * 1024 * 1024 * 1024);

    let runtimes = Runtimes::with_base(dir.path(), upstream.base()).expect("a client");
    let progress = runtimes.watch(21);
    let refusal = runtimes.install(21).await.expect_err("the announcement is impossible");

    assert_eq!(refusal.code(), "java_download_announced_oversized");
    assert!(refusal.to_string().contains("8589934592"), "{refusal}");
    assert!(refusal.to_string().contains("134217728"), "{refusal}");
    assert_eq!(upstream.asked(), 1, "the question was asked");
    assert_eq!(upstream.served(), 0, "and the answer cost not one byte of archive");
    assert_eq!(progress.total(), 0, "the bar never went to downloading");
    assert_eq!(progress.done(), 0);
    assert!(!dir.path().join("runtimes").join(".java-21.new").exists());
    assert!(!dir.path().join("runtimes").join("java-21").exists());
}

#[test]
fn the_ceiling_follows_the_announced_size_and_stops_at_the_fixed_roof() {
    assert_eq!(super::ceiling(41_851_657), 41_851_657 + 1024 * 1024);
    assert_eq!(super::ceiling(0), 128 * 1024 * 1024, "an answer without a size gets the roof");
    assert_eq!(super::ceiling(u64::MAX), 128 * 1024 * 1024, "and so does one that lies big");
}

#[tokio::test]
#[ignore = "downloads 40 MB of Java 8 from Adoptium"]
async fn live_java_8_comes_down_from_adoptium_and_runs() {
    let dir = a_data_dir();
    let runtimes = Runtimes::new(dir.path()).expect("a client");
    let progress = runtimes.watch(8);

    let installed = runtimes.install(8).await.expect("Java 8 from Adoptium");

    assert!(installed.fresh);
    assert_eq!(installed.runtime.major, 8);
    assert_eq!(installed.runtime.vendor, JreVendor::Temurin);
    assert_eq!(installed.runtime.source, Source::Managed);
    assert_eq!(installed.home, dir.path().join("runtimes").join("java-8"));

    let shown = version_of(&installed.home.join("bin").join("java"));
    assert!(shown.contains("1.8.0"), "{shown}");
    assert!(shown.contains("OpenJDK"), "{shown}");
    harness::nothing_is_loose(&installed.home);

    assert!(progress.total() > 20_000_000, "{} bytes announced", progress.total());
    assert_eq!(progress.stage(), Stage::Done);
    assert!(!runtimes.install(8).await.expect("the one that is there").fresh);
}

const OLDER: &str = "21.0.11+9";

fn a_runtime_on_disk(home: &Path, version: &str) {
    let binary = home.join("bin").join("java");
    std::fs::create_dir_all(home.join("bin")).expect("a runtime directory");
    std::fs::write(&binary, format!("#!/bin/sh\necho 'openjdk version \"{version}\"' 1>&2\n"))
        .expect("a launcher");
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755))
        .expect("a runnable launcher");
    std::fs::write(
        home.join("release"),
        format!("IMPLEMENTOR=\"Eclipse Adoptium\"\nJAVA_VERSION=\"{version}\"\n"),
    )
    .expect("a release file");
}

#[tokio::test]
async fn a_runtime_that_a_crash_left_standing_aside_is_put_back_before_anything_is_swept() {
    let dir = a_data_dir();
    let home = dir.path().join("runtimes").join("java-21");
    let staging = dir.path().join("runtimes").join(".java-21.new");
    a_runtime_on_disk(&staging.join("previous"), OLDER);

    let upstream = FakeAdoptium::started().await;
    upstream.offer(21, VERSION, harness::a_jre(VERSION));
    let runtimes = Runtimes::with_base(dir.path(), upstream.base()).expect("a client");
    let installed = runtimes.install(21).await.expect("the one that survived");

    assert!(!installed.fresh, "it was rescued, not fetched again");
    assert_eq!(installed.home, home);
    assert_eq!(installed.runtime.version, OLDER);
    assert!(version_of(&home.join("bin").join("java")).contains(OLDER));
    assert_eq!(upstream.asked(), 0, "and Adoptium was not asked for what we already had");
    assert!(!staging.exists(), "the wreckage went afterwards");
}

#[tokio::test]
async fn a_crash_with_the_new_tree_already_standing_by_finishes_the_swap() {
    let dir = a_data_dir();
    let home = dir.path().join("runtimes").join("java-21");
    let staging = dir.path().join("runtimes").join(".java-21.new");
    a_runtime_on_disk(&staging.join("previous"), OLDER);
    a_runtime_on_disk(&staging.join("ready"), VERSION);

    let upstream = FakeAdoptium::started().await;
    upstream.offer(21, VERSION, harness::a_jre(VERSION));
    let runtimes = Runtimes::with_base(dir.path(), upstream.base()).expect("a client");
    let installed = runtimes.install(21).await.expect("the one that was ready to move in");

    assert!(!installed.fresh);
    assert_eq!(installed.runtime.version, VERSION, "the checked new tree wins over the old one");
    assert!(version_of(&home.join("bin").join("java")).contains(VERSION));
    assert_eq!(upstream.asked(), 0);
    assert!(!staging.exists());
}

#[tokio::test]
async fn a_runtime_that_stands_where_it_belongs_is_not_pushed_aside_by_what_a_crash_left() {
    let dir = a_data_dir();
    let home = dir.path().join("runtimes").join("java-21");
    let staging = dir.path().join("runtimes").join(".java-21.new");
    a_runtime_on_disk(&home, VERSION);
    a_runtime_on_disk(&staging.join("previous"), OLDER);

    let upstream = FakeAdoptium::started().await;
    upstream.offer(21, VERSION, harness::a_jre(VERSION));
    let runtimes = Runtimes::with_base(dir.path(), upstream.base()).expect("a client");
    let installed = runtimes.install(21).await.expect("the one that stands there");

    assert!(!installed.fresh);
    assert_eq!(installed.runtime.version, VERSION, "the one in place stays in place");
    assert!(version_of(&home.join("bin").join("java")).contains(VERSION));
    assert_eq!(upstream.asked(), 0);
    assert!(!staging.exists(), "and the leftover was swept, not installed");
}


const NOBODY: u32 = 65534;
const A_CHILD: &str = "CRAFTPANEL_JAVA_UNDER_A_LOOSE_UMASK";

fn mode_of(path: &Path) -> u32 {
    std::fs::symlink_metadata(path).expect("the entry").permissions().mode() & 0o7777
}

fn as_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

fn a_stranger_runs(binary: &Path) -> Option<String> {
    use std::os::unix::process::CommandExt;

    if !as_root() {
        return None;
    }
    let shown = std::process::Command::new(binary)
        .arg("-version")
        .uid(NOBODY)
        .gid(NOBODY)
        .output()
        .expect("an account of the machine's own runs the launcher");
    Some(String::from_utf8_lossy(&shown.stderr).into_owned())
}

fn under_a_umask_of_its_own(test: &str) -> bool {
    if std::env::var_os(A_CHILD).is_some() {
        return false;
    }
    let Some(name) = module_path!().split_once("::").map(|(_, rest)| rest) else {
        panic!("a test module always sits below its crate");
    };
    let ran = std::process::Command::new(std::env::current_exe().expect("this binary"))
        .args(["--exact", &format!("{name}::{test}")])
        .arg("--nocapture")
        .env(A_CHILD, "0000")
        .output()
        .expect("this binary again, in a process of its own");
    let said = String::from_utf8_lossy(&ran.stdout).into_owned()
        + &String::from_utf8_lossy(&ran.stderr);
    assert!(ran.status.success(), "{said}");
    assert!(said.contains("1 passed"), "the child ran no test at all: {said}");
    true
}

#[tokio::test]
async fn a_runtimes_directory_that_was_already_shut_is_opened_before_java_lands_behind_it() {
    let dir = a_data_dir();
    let above = dir.path().join("runtimes");
    std::fs::create_dir_all(&above).expect("the directory an earlier run left standing");
    std::fs::set_permissions(&above, std::fs::Permissions::from_mode(0o700))
        .expect("and it lets nobody else in");

    let upstream = FakeAdoptium::started().await;
    upstream.offer(21, VERSION, harness::a_jre(VERSION));
    let runtimes = Runtimes::with_base(dir.path(), upstream.base()).expect("a client");
    let installed = runtimes.install(21).await.expect("a runtime");

    assert_eq!(mode_of(&above), 0o755, "the way to bin/java is open for every account");
    let binary = installed.home.join("bin").join("java");
    if let Some(said) = a_stranger_runs(&binary) {
        assert!(said.contains(VERSION), "an account of its own could not run it: {said}");
    }
}

#[tokio::test]
async fn a_runtimes_directory_that_belongs_to_someone_else_stops_the_install_and_says_why() {
    if !as_root() {
        eprintln!("skipped: only root can hand a directory to another account");
        return;
    }

    let dir = a_data_dir();
    let above = dir.path().join("runtimes");
    std::fs::create_dir_all(&above).expect("a directory the operator made");
    std::fs::set_permissions(&above, std::fs::Permissions::from_mode(0o700))
        .expect("shut to everyone else");
    std::os::unix::fs::chown(&above, Some(NOBODY), Some(NOBODY)).expect("and not ours");

    let upstream = FakeAdoptium::started().await;
    upstream.offer(21, VERSION, harness::a_jre(VERSION));
    let runtimes = Runtimes::with_base(dir.path(), upstream.base()).expect("a client");
    let refusal = runtimes.install(21).await.expect_err("nothing that cannot be started");

    assert_eq!(refusal.code(), "java_runtime_unreachable");
    let said = refusal.to_string();
    assert!(said.contains("chmod o+rx"), "{said}");
    assert!(said.contains(&above.display().to_string()), "{said}");
    assert!(said.contains("0700") && said.contains(&NOBODY.to_string()), "{said}");
    assert_eq!(mode_of(&above), 0o700, "and another account's directory is left as it is");
    assert_eq!(upstream.asked(), 0, "the refusal comes before Adoptium is asked for anything");
    assert!(!above.join("java-21").exists());
}

#[tokio::test]
async fn a_data_directory_a_game_account_cannot_enter_is_named_and_not_written_into() {
    let dir = a_data_dir();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o750))
        .expect("a data directory the panel keeps to itself");

    let upstream = FakeAdoptium::started().await;
    upstream.offer(21, VERSION, harness::a_jre(VERSION));
    let runtimes = Runtimes::with_base(dir.path(), upstream.base()).expect("a client");
    let refusal = runtimes.install(21).await.expect_err("no runtime nobody can start");

    assert_eq!(refusal.code(), "java_runtime_unreachable");
    assert!(refusal.to_string().contains(&dir.path().display().to_string()), "{refusal}");
    assert_eq!(upstream.asked(), 0);
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755))
        .expect("open again, so the scratch can be swept up");
}

#[tokio::test]
async fn the_staging_shuts_everyone_else_out_while_the_bytes_come_down() {
    if under_a_umask_of_its_own("the_staging_shuts_everyone_else_out_while_the_bytes_come_down") {
        return;
    }
    unsafe { libc::umask(0o000) };

    let dir = a_data_dir();
    let upstream = FakeAdoptium::started().await;
    upstream.offer(21, VERSION, harness::a_jre(VERSION));
    upstream.shut();

    let runtimes = Arc::new(Runtimes::with_base(dir.path(), upstream.base()).expect("a client"));
    let task = {
        let runtimes = Arc::clone(&runtimes);
        tokio::spawn(async move { runtimes.install(21).await })
    };

    let staging = dir.path().join("runtimes").join(".java-21.new");
    for _ in 0..500 {
        if staging.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(mode_of(&staging), 0o700, "nobody else can so much as look into the staging");

    upstream.open();
    let home = task.await.expect("the task").expect("a runtime").home;
    assert!(!staging.exists(), "and it is gone when the runtime stands");
    assert_eq!(mode_of(&dir.path().join("runtimes")), 0o755);
    assert_eq!(mode_of(&home), 0o755);
}

#[tokio::test]
async fn the_archive_on_its_way_in_takes_the_writers_mode_and_not_the_umasks() {
    let name = "the_archive_on_its_way_in_takes_the_writers_mode_and_not_the_umasks";
    if under_a_umask_of_its_own(name) {
        return;
    }
    unsafe { libc::umask(0o000) };

    let dir = a_data_dir();
    let upstream = FakeAdoptium::started().await;
    upstream.offer(21, VERSION, harness::a_jre(VERSION));
    let runtimes = Runtimes::with_base(dir.path(), upstream.base()).expect("a client");

    let staging = dir.path().join("runtimes").join(".java-21.new");
    super::make_reachable(dir.path(), 21).expect("runtimes/ stands open");
    super::empty_out(&staging).expect("a staging of its own");
    let home = runtimes.home(21);
    let progress = runtimes.watch(21);
    runtimes
        .fetch(21, super::Arch::here().expect("a machine we serve"), &staging, &home, &progress)
        .await
        .expect("a runtime");

    assert_eq!(mode_of(&staging), 0o700, "the staging");
    assert_eq!(mode_of(&staging.join("archive.tar.gz")), 0o600, "the archive inside it");
    assert_eq!(mode_of(&dir.path().join("runtimes")), 0o755, "and runtimes/ above it");
    harness::nothing_is_loose(&home);
}

#[tokio::test]
async fn a_tree_left_in_a_staging_anyone_could_write_to_is_swept_up_rather_than_installed() {
    let dir = a_data_dir();
    let home = dir.path().join("runtimes").join("java-21");
    let staging = dir.path().join("runtimes").join(".java-21.new");
    a_runtime_on_disk(&staging.join("ready"), OLDER);
    std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o777))
        .expect("a staging anyone may write into");

    let upstream = FakeAdoptium::started().await;
    upstream.offer(21, VERSION, harness::a_jre(VERSION));
    let runtimes = Runtimes::with_base(dir.path(), upstream.base()).expect("a client");
    let installed = runtimes.install(21).await.expect("a runtime");

    assert!(installed.fresh, "what a stranger could have laid there is fetched again");
    assert_eq!(installed.runtime.version, VERSION);
    assert!(version_of(&home.join("bin").join("java")).contains(VERSION));
    assert!(!staging.exists());
}

#[tokio::test]
async fn a_tree_another_account_left_in_the_staging_is_swept_up_rather_than_installed() {
    if !as_root() {
        eprintln!("skipped: only root can hand a directory to another account");
        return;
    }

    let dir = a_data_dir();
    let home = dir.path().join("runtimes").join("java-21");
    let staging = dir.path().join("runtimes").join(".java-21.new");
    let planted = staging.join("ready");
    a_runtime_on_disk(&planted, OLDER);
    for step in [&staging, &planted] {
        std::os::unix::fs::chown(step, Some(NOBODY), Some(NOBODY)).expect("another account's");
    }

    let upstream = FakeAdoptium::started().await;
    upstream.offer(21, VERSION, harness::a_jre(VERSION));
    let runtimes = Runtimes::with_base(dir.path(), upstream.base()).expect("a client");
    let installed = runtimes.install(21).await.expect("a runtime");

    assert!(installed.fresh, "a tree another account left is no runtime of ours");
    assert_eq!(installed.runtime.version, VERSION);
    assert!(version_of(&home.join("bin").join("java")).contains(VERSION));
    assert!(!staging.exists());
}

fn a_jre_nested(steps: usize) -> Vec<u8> {
    let deep = vec!["d"; steps].join("/");
    let release = format!(
        "IMPLEMENTOR=\"Eclipse Adoptium\"\nJAVA_VERSION=\"{VERSION}\"\nOS_ARCH=\"x86_64\"\n"
    );
    let launcher = format!("#!/bin/sh\necho 'openjdk version \"{VERSION}\"' 1>&2\n");

    harness::tarball(|builder| {
        harness::file(builder, &format!("{}/bin/java", root()), launcher.as_bytes(), 0o755);
        harness::file(builder, &format!("{}/release", root()), release.as_bytes(), 0o644);
        let name = format!("{}/{deep}/deep.txt", root());
        harness::file(builder, &name, b"as deep as it goes", 0o644);
    })
}

#[tokio::test]
async fn an_entry_nested_deeper_than_any_runtime_is_refused_before_it_is_dug() {
    let (refusal, dir) = refused(a_jre_nested(15)).await;

    assert_eq!(refusal.code(), "java_archive_rejected");
    assert!(refusal.to_string().contains("nested deeper than 16 directories"), "{refusal}");
    assert!(!dir.path().join("runtimes").join("java-21").exists());
}

#[tokio::test]
async fn an_entry_at_the_deepest_step_still_allowed_is_laid_down() {
    let dir = a_data_dir();
    let upstream = FakeAdoptium::started().await;
    upstream.offer(21, VERSION, a_jre_nested(14));

    let runtimes = Runtimes::with_base(dir.path(), upstream.base()).expect("a client");
    let installed = runtimes.install(21).await.expect("a runtime");

    let deep = installed.home.join(vec!["d"; 14].join("/")).join("deep.txt");
    assert_eq!(std::fs::read(&deep).expect("the deepest file"), b"as deep as it goes");
}

#[tokio::test]
async fn a_runtime_twice_as_deep_as_any_temurin_is_still_laid_down_whole() {
    let dir = a_data_dir();
    let steps = 2 * A_TEMURIN_IS_DEEP - 2;
    let upstream = FakeAdoptium::started().await;
    upstream.offer(21, VERSION, a_jre_nested(steps));

    let runtimes = Runtimes::with_base(dir.path(), upstream.base()).expect("a client");
    let installed = runtimes.install(21).await.expect("a runtime");

    let deep = installed.home.join(vec!["d"; steps].join("/"));
    assert_eq!(
        std::fs::read(deep.join("deep.txt")).expect("the deepest file"),
        b"as deep as it goes"
    );
}

#[tokio::test]
async fn a_runtimes_directory_anyone_may_write_into_is_refused_rather_than_filled() {
    let dir = a_data_dir();
    let above = dir.path().join("runtimes");
    std::fs::create_dir_all(&above).expect("a directory somebody opened up");
    std::fs::set_permissions(&above, std::fs::Permissions::from_mode(0o777))
        .expect("to everyone on the machine");

    let upstream = FakeAdoptium::started().await;
    upstream.offer(21, VERSION, harness::a_jre(VERSION));
    let runtimes = Runtimes::with_base(dir.path(), upstream.base()).expect("a client");
    let refusal = runtimes.install(21).await.expect_err("no runtime anyone could swap");

    assert_eq!(refusal.code(), "java_runtime_exposed");
    let said = refusal.to_string();
    assert!(said.contains("chmod o-w"), "{said}");
    assert!(said.contains(&above.display().to_string()), "{said}");
    assert_eq!(upstream.asked(), 0, "and Adoptium was not asked for anything");
    assert!(!above.join("java-21").exists());
}

