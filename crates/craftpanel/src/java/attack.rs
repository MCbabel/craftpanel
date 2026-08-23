#![cfg(test)]

use std::os::unix::fs::MetadataExt;
use std::path::Path;

use super::harness::{self, a_data_dir, FakeAdoptium};
use super::progress::Progress;
use super::{unpack, Runtimes};
use crate::settings::runtimes::{discover, read_home, Search, Source};

const VERSION: &str = "21.0.12+7";
const A_FILE_NO_JAVA_HOLDS: u64 = 300 * 1024 * 1024;
const A_NAME_NO_PATH_HOLDS: u64 = 3 * 1024 * 1024 * 1024;
const A_RELEASE_NO_ONE_WROTE: u64 = 1_572_864_056;
const TREES_NO_RUNTIME_PLANTS: usize = 512;
const A_NESTING_NO_RUNTIME_HAS: usize = 250;
const THE_DEEPEST_A_RUNTIME_MAY_BE: usize = 16;
const THE_MOST_A_RUNTIME_MAY_MAKE: usize = 4_096;

fn root() -> String {
    format!("jdk-{VERSION}-jre")
}

fn release() -> Vec<u8> {
    format!("IMPLEMENTOR=\"Eclipse Adoptium\"\nJAVA_VERSION=\"{VERSION}\"\n").into_bytes()
}

#[tokio::test]
async fn a_file_larger_than_any_file_a_temurin_holds_is_never_written() {
    let dir = a_data_dir();
    let upstream = FakeAdoptium::started().await;
    let archive = harness::tarball(|builder| {
        harness::file(builder, &format!("{}/bin/java", root()), b"#!/bin/sh\n", 0o755);
        harness::padded_file(
            builder,
            &format!("{}/release", root()),
            &release(),
            A_FILE_NO_JAVA_HOLDS,
            0o644,
        );
    });

    upstream.offer(21, VERSION, archive);
    let runtimes = Runtimes::with_base(dir.path(), upstream.base()).expect("a client");
    let progress = runtimes.watch(21);
    let refusal =
        runtimes.install(21).await.expect_err("300 MiB in one file is no part of a runtime");

    assert_eq!(refusal.code(), "java_archive_rejected", "{refusal}");
    assert!(refusal.to_string().contains("in one file"), "{refusal}");
    assert!(
        !dir.path().join("runtimes").join("java-21").exists(),
        "a runtime was laid down all the same"
    );
    assert!(
        progress.done() < 128 * 1024,
        "{} bytes of the archive were pulled through before it was refused",
        progress.done()
    );
}

#[test]
fn a_name_that_announces_gigabytes_is_refused_before_the_tar_crate_reads_it() {
    let dir = a_data_dir();
    let carried = harness::noise(8 * 1024 * 1024);
    let archive = harness::tarball(|builder| {
        harness::raw_announcing(
            builder,
            b"././@LongLink",
            tar::EntryType::GNULongName,
            A_NAME_NO_PATH_HOLDS,
            &carried,
        );
        harness::file(builder, &format!("{}/bin/java", root()), b"#!/bin/sh\n", 0o755);
    });

    let at = dir.path().join("a-long-name.tar.gz");
    std::fs::write(&at, archive).expect("an archive on disk");

    let progress = Progress::default();
    let refusal = unpack::tree(&at, &dir.path().join("tree"), 21, &progress)
        .expect_err("three gigabytes is no name");

    assert_eq!(refusal.code(), "java_archive_rejected", "{refusal}");
    assert!(refusal.to_string().contains("of name"), "{refusal}");
    assert!(
        progress.done() < 128 * 1024,
        "{} bytes were pulled through for a name that was never going to be one",
        progress.done()
    );
}

#[test]
fn a_release_file_of_gigabytes_is_no_release_file_and_is_never_read_whole() {
    let dir = a_data_dir();
    let home = dir.path().join("runtimes").join("java-21");
    std::fs::create_dir_all(home.join("bin")).expect("a runtime shaped directory");
    std::fs::write(home.join("bin").join("java"), "#!/bin/sh\n").expect("a launcher");

    let at = home.join("release");
    std::fs::write(&at, release()).expect("a release file that reads like one");
    std::fs::OpenOptions::new()
        .write(true)
        .open(&at)
        .expect("the release file again")
        .set_len(A_RELEASE_NO_ONE_WROTE)
        .expect("a file that is mostly hole");

    assert_eq!(read_home(&home, Source::Managed), None, "1.5 GiB of release was read");

    let managed: Vec<_> = discover(dir.path(), &Search::nowhere())
        .into_iter()
        .filter(|found| found.source == Source::Managed)
        .collect();
    assert!(managed.is_empty(), "{managed:?}");
}

fn a_forest(trees: usize, steps: usize) -> Vec<u8> {
    let spine = vec!["d"; steps - 1].join("/");
    harness::tarball(|builder| {
        harness::file(builder, &format!("{}/bin/java", root()), b"#!/bin/sh\n", 0o755);
        harness::file(builder, &format!("{}/release", root()), &release(), 0o644);
        for tree in 0..trees {
            harness::file(builder, &format!("{}/{tree}/{spine}/leaf.txt", root()), b"x", 0o644);
        }
    })
}

fn dug_into(dir: &Path) -> (usize, u64) {
    let mut inodes = 0usize;
    let mut held = 0u64;
    for found in walkdir::WalkDir::new(dir).follow_links(false).min_depth(1) {
        let found = found.expect("every step of what was dug");
        inodes += 1;
        held += found.path().symlink_metadata().expect("the entry").blocks() * 512;
    }
    (inodes, held)
}

#[test]
fn a_forest_of_directories_out_of_a_handful_of_names_is_refused_before_it_is_dug() {
    let dir = a_data_dir();
    let archive = a_forest(TREES_NO_RUNTIME_PLANTS, A_NESTING_NO_RUNTIME_HAS);
    let asked = TREES_NO_RUNTIME_PLANTS * (A_NESTING_NO_RUNTIME_HAS + 1);
    let at = dir.path().join("a-forest.tar.gz");
    let into = dir.path().join("tree");
    std::fs::write(&at, &archive).expect("an archive on disk");
    assert!(
        asked > 128_000 && archive.len() < 64 * 1024,
        "{asked} inodes out of {} bytes is not the archive that was reported",
        archive.len()
    );

    let refusal = unpack::tree(&at, &into, 21, &Progress::default())
        .expect_err("250 steps of nesting are no part of a runtime");

    assert_eq!(refusal.code(), "java_archive_rejected", "{refusal}");
    assert!(
        refusal.to_string().contains(&format!("deeper than {THE_DEEPEST_A_RUNTIME_MAY_BE}")),
        "{refusal}"
    );

    let (inodes, held) = dug_into(&into);
    assert_eq!(inodes, 3, "bin, bin/java and release are all of it, and none of the forest");
    assert!(
        held < 64 * 1024,
        "{held} bytes lie under a tree that was refused, where {asked} inodes were asked for"
    );
}

#[tokio::test]
async fn a_forest_with_a_launcher_beside_it_is_still_a_forest_and_is_not_installed() {
    let dir = a_data_dir();
    let upstream = FakeAdoptium::started().await;
    upstream.offer(21, VERSION, a_forest(TREES_NO_RUNTIME_PLANTS, A_NESTING_NO_RUNTIME_HAS));

    let runtimes = Runtimes::with_base(dir.path(), upstream.base()).expect("a client");
    let refusal =
        runtimes.install(21).await.expect_err("a launcher does not make a forest a runtime");

    assert_eq!(refusal.code(), "java_archive_rejected", "{refusal}");
    let (inodes, held) = dug_into(&dir.path().join("runtimes"));
    assert_eq!(inodes, 0, "runtimes/ holds what the forest left: {inodes} entries, {held} bytes");
    assert!(!dir.path().join("runtimes").join("java-21").exists());
}

#[test]
fn the_directories_between_the_names_are_counted_the_way_the_names_themselves_are() {
    let dir = a_data_dir();
    let trees = 2 * TREES_NO_RUNTIME_PLANTS;
    let steps = THE_DEEPEST_A_RUNTIME_MAY_BE - 2;
    let archive = a_forest(trees, steps);
    let asked = trees * (steps + 1);
    let at = dir.path().join("a-thicket.tar.gz");
    let into = dir.path().join("tree");
    std::fs::write(&at, &archive).expect("an archive on disk");

    let refusal = unpack::tree(&at, &into, 21, &Progress::default())
        .expect_err("names within the depth still dig more directories than a runtime holds");

    assert_eq!(refusal.code(), "java_archive_rejected", "{refusal}");
    assert!(refusal.to_string().contains("entries and directories"), "{refusal}");

    let (inodes, held) = dug_into(&into);
    assert!(
        inodes < THE_MOST_A_RUNTIME_MAY_MAKE + 2 * THE_DEEPEST_A_RUNTIME_MAY_BE,
        "{inodes} of the {asked} inodes the archive asked for were dug, holding {held} bytes"
    );
    assert!(held < 32 * 1024 * 1024, "{held} bytes for {trees} names of {steps} steps");
}

