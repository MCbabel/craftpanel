use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::backups::archive::Progress;
use crate::model::{
    BackupLocation, DriveAccountState, DriveFileState, Id, PanelRole, Timestamp,
};
use crate::ops::testing::{a_server, a_user, cut_off, schema};

use super::harness::{self, DataDir, FakeGoogle};
use super::http::DriveError;
use super::{Recorded, Stored, SESSION_LIFE};

const NO_WRITE_ACCESS: &str = r#"{"error":{"code":403,"errors":[{"reason":
    "insufficientFilePermissions","domain":"global","message":"The user does not have
    sufficient permissions for this file."}],"message":"Insufficient permissions."}}"#;

const USER_RATE_LIMIT: &str = r#"{"error":{"code":403,"errors":[{"reason":"userRateLimitExceeded",
    "domain":"usageLimits","message":"User rate limit exceeded."}],
    "message":"User rate limit exceeded."}}"#;

struct Siege {
    pool: sqlx::SqlitePool,
    dir: DataDir,
    google: FakeGoogle,
    drive: Arc<super::Drive>,
    anna: Id,
    server: Id,
    backup: Id,
    archive: std::path::PathBuf,
    whole: Vec<u8>,
    progress: Arc<Progress>,
}

impl Siege {
    async fn of(chunks: u64) -> Self {
        let pool = schema().await;
        let dir = DataDir::new();
        let google = FakeGoogle::started().await;
        let drive = harness::service(&pool, &dir, &google);
        harness::with_credentials(&drive).await;

        let anna = a_user(&pool, PanelRole::User).await;
        let server = a_server(&pool, anna).await;
        drive.of(anna).write_token("1//a-token").await;

        let whole = filler((super::upload::CHUNK * chunks + 4096) as usize, 1);
        tokio::fs::create_dir_all(dir.path()).await.expect("a place for the archive");
        let archive = dir.path().join("monday.tar.zst");
        tokio::fs::write(&archive, &whole).await.expect("an archive on the disk");
        let backup = crate::backups::store::insert(
            &pool,
            server,
            "Monday",
            false,
            BackupLocation::Drive,
        )
        .await
        .expect("a backup row")
        .id;

        Self {
            pool,
            dir,
            google,
            drive,
            anna,
            server,
            backup,
            archive,
            whole,
            progress: Arc::default(),
        }
    }

    async fn send(&self) -> std::result::Result<Stored, DriveError> {
        self.send_with(&self.drive).await
    }

    async fn send_with(
        &self,
        drive: &Arc<super::Drive>,
    ) -> std::result::Result<Stored, DriveError> {
        drive
            .upload_archive(
                self.server,
                self.backup,
                &self.archive,
                self.whole.len() as u64,
                "monday.tar.zst",
                &self.progress,
            )
            .await
    }

    fn after_a_restart(&self) -> Arc<super::Drive> {
        harness::service(&self.pool, &self.dir, &self.google)
    }

    async fn stop_after(&self, chunks: usize) {
        let drive = Arc::clone(&self.drive);
        let archive = self.archive.clone();
        let (server, backup, size) = (self.server, self.backup, self.whole.len() as u64);
        let sending = tokio::spawn(async move {
            let progress = Arc::new(Progress::default());
            let _ = drive
                .upload_archive(server, backup, &archive, size, "monday.tar.zst", &progress)
                .await;
        });
        for _ in 0..2000 {
            if self.google.chunks_seen() >= chunks {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(self.google.chunks_seen() >= chunks, "Google never saw {chunks} chunks");
        cut_off(sending, &self.pool).await;
    }

    fn address(&self) -> std::path::PathBuf {
        self.dir
            .path()
            .join("drive")
            .join(self.anna.to_string())
            .join("sessions")
            .join(self.backup.to_string())
    }

    async fn session(&self) -> Option<super::store::Upload> {
        super::store::upload_of(&self.pool, self.backup).await.expect("no error")
    }

    async fn a_second_backup(&self) -> (Id, std::path::PathBuf, Vec<u8>) {
        let whole = filler(self.whole.len(), 2);
        let archive = self.dir.path().join("tuesday.tar.zst");
        tokio::fs::write(&archive, &whole).await.expect("a second archive on the disk");
        let backup = crate::backups::store::insert(
            &self.pool,
            self.server,
            "Tuesday",
            false,
            BackupLocation::Drive,
        )
        .await
        .expect("a backup row")
        .id;
        (backup, archive, whole)
    }

    fn archives_in_the_drive(&self) -> Vec<harness::StoredFile> {
        self.google.files().into_iter().filter(|file| !file.folder).collect()
    }
}

fn filler(bytes: usize, seed: u64) -> Vec<u8> {
    (0..bytes)
        .map(|at| ((at as u64).wrapping_mul(2_654_435_761).wrapping_add(seed) >> 11) as u8)
        .collect()
}

fn md5_of(bytes: &[u8]) -> String {
    use md5::Digest;
    let mut digest = md5::Md5::new();
    digest.update(bytes);
    hex::encode(digest.finalize())
}

#[tokio::test]
async fn a_rate_limit_on_one_chunk_is_waited_out_and_the_archive_arrives_whole() {
    let siege = Siege::of(2).await;
    siege.google.turn_away("upload/chunk", 1, 429, None);

    let stored = siege.send().await.expect("a 429 is a moment, not an answer");

    let file = siege.google.file_of_backup(siege.backup).expect("the archive in her Drive");
    assert_eq!(file.bytes, siege.whole, "the archive in the Drive is not the one on the disk");
    assert_eq!(stored.md5.as_deref(), Some(md5_of(&siege.whole).as_str()));
    assert_eq!(
        siege.google.times_called("upload/chunk"),
        4,
        "three chunks and the one that was turned away: {:?}",
        siege.google.calls()
    );
    assert!(siege.session().await.is_none(), "a spent session is not kept");
}

#[tokio::test]
async fn three_rate_limits_in_a_row_on_the_same_chunk_are_still_ridden_out() {
    let siege = Siege::of(1).await;
    siege.google.turn_away("upload/chunk", 3, 429, None);

    let started = Instant::now();
    siege.send().await.expect("three bad moments in a row are still bad moments");
    let waited = started.elapsed();

    assert_eq!(
        siege.google.file_of_backup(siege.backup).expect("the archive").bytes,
        siege.whole
    );
    assert_eq!(siege.google.times_called("upload/chunk"), 5, "{:?}", siege.google.calls());
    assert!(waited < Duration::from_secs(10), "the panel hung for {waited:?} over three 429s");
}

#[tokio::test]
async fn a_google_that_says_429_for_ever_gives_up_and_keeps_the_session_for_the_next_run() {
    let siege = Siege::of(1).await;
    siege.google.turn_away("upload/chunk", 999, 429, None);

    let started = Instant::now();
    let err = siege.send().await.expect_err("a night of 429 is a failed run");
    let waited = started.elapsed();

    assert!(matches!(err, DriveError::RateLimited), "{err:?}");
    assert_eq!(
        siege.google.times_called("upload/chunk"),
        super::retry::Backoff::HURRIED.tries as usize,
        "the ceiling on the tries was not held to: {:?}",
        siege.google.calls()
    );
    assert!(
        waited < super::retry::Backoff::HURRIED.budget * 3,
        "the run was held for {waited:?} on a budget of {:?}",
        super::retry::Backoff::HURRIED.budget
    );
    assert!(siege.archives_in_the_drive().is_empty(), "half an archive was left in the Drive");
    assert!(siege.session().await.is_some(), "the half-sent upload is worth carrying on");
    assert!(siege.address().exists(), "and its address is on the disk");
}

#[tokio::test]
async fn the_second_a_rate_limit_asks_for_is_the_second_it_gets() {
    let siege = Siege::of(0).await;
    siege.google.turn_away("upload/chunk", 1, 429, Some("1"));

    let started = Instant::now();
    siege.send().await.expect("the second try got through");

    assert!(
        started.elapsed() >= Duration::from_millis(900),
        "Google asked for a second and got {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn a_retry_after_of_a_whole_day_is_not_waited_out_but_given_up_on() {
    let siege = Siege::of(0).await;
    siege.google.turn_away("upload/chunk", 999, 429, Some("86400"));

    let started = Instant::now();
    let err = siege.send().await.expect_err("a day is not a wait, it is a hang");
    let waited = started.elapsed();

    assert!(matches!(err, DriveError::RateLimited), "{err:?}");
    assert_eq!(
        siege.google.times_called("upload/chunk"),
        1,
        "a wait longer than the whole budget was tried again anyway"
    );
    assert!(waited < Duration::from_secs(5), "the run was held for {waited:?} by one header");
}

#[tokio::test]
async fn a_five_hundred_and_a_five_oh_three_in_the_middle_are_both_ridden_out() {
    for status in [500u16, 503] {
        let siege = Siege::of(1).await;
        siege.google.turn_away("upload/chunk", 2, status, None);

        siege.send().await.unwrap_or_else(|err| panic!("a {status} ended the run: {err}"));
        assert_eq!(
            siege.google.file_of_backup(siege.backup).expect("the archive").bytes,
            siege.whole,
            "the archive that survived a {status} is not the one on the disk"
        );
    }
}

#[tokio::test]
async fn a_five_hundred_while_the_session_is_being_opened_is_tried_again() {
    let siege = Siege::of(0).await;
    siege.google.turn_away("upload/begin", 2, 500, None);

    siege.send().await.expect("a 500 on the way in is a bad moment like any other");

    assert_eq!(siege.google.times_called("upload/begin"), 3, "{:?}", siege.google.calls());
    assert_eq!(siege.google.sessions_open(), 1, "every try left a session lying open at Google");
}

#[tokio::test]
async fn a_google_that_will_not_open_a_session_at_all_leaves_nothing_behind() {
    let siege = Siege::of(0).await;
    siege.google.turn_away("upload/begin", 999, 500, None);

    let err = siege.send().await.expect_err("no session, no upload");

    assert!(matches!(err, DriveError::Refused { status: 500, .. }), "{err:?}");
    assert_eq!(siege.google.sessions_open(), 0);
    assert!(siege.session().await.is_none(), "a session row without a session at Google");
    assert!(!siege.address().exists(), "an address that was never handed out");
    assert!(siege.archives_in_the_drive().is_empty());
}

#[tokio::test]
async fn a_full_drive_in_the_middle_of_a_chunk_is_never_asked_a_second_time() {
    let siege = Siege::of(1).await;
    siege.google.fill_the_drive();

    let err = siege.send().await.expect_err("a full Drive does not empty itself");

    assert_eq!(err.operation_code(), "drive_quota_exceeded");
    assert_eq!(
        siege.google.times_called("upload/chunk"),
        1,
        "a full Drive was asked again and again: {:?}",
        siege.google.calls()
    );
    assert!(siege.archives_in_the_drive().is_empty());
}

#[tokio::test]
async fn a_five_oh_seven_in_the_middle_of_a_chunk_is_no_bad_moment_either() {
    let siege = Siege::of(1).await;
    siege.google.fail_chunk(1, 507);

    let err = siege.send().await.expect_err("there is no room, and waiting makes none");

    assert!(matches!(err, DriveError::Refused { status: 507, .. }), "{err:?}");
    assert_eq!(siege.google.times_called("upload/chunk"), 1, "{:?}", siege.google.calls());
}

#[tokio::test]
async fn a_checksum_that_does_not_match_leaves_nothing_in_the_drive_to_be_found_later() {
    let siege = Siege::of(1).await;
    siege.google.garble_what_arrives();

    let err = siege.send().await.expect_err("what lies there is not what left this machine");

    assert_eq!(err.operation_code(), "drive_checksum_mismatch");
    assert!(
        siege.archives_in_the_drive().is_empty(),
        "the mangled archive stayed in her Drive: {:?}",
        siege.archives_in_the_drive()
    );
    assert!(siege.session().await.is_none(), "the spent session was kept");
    assert!(siege.google.times_called("files/delete") >= 1);
}

#[tokio::test]
async fn an_archive_nobody_may_trust_that_google_will_not_delete_is_swept_up_afterwards() {
    let siege = Siege::of(0).await;
    siege.google.garble_what_arrives();
    siege.google.turn_away_with_body("files/delete", 1, 403, NO_WRITE_ACCESS);

    let err = siege.send().await.expect_err("a mangled archive is not a backup");
    assert_eq!(err.operation_code(), "drive_checksum_mismatch");
    assert_eq!(
        siege.archives_in_the_drive().len(),
        1,
        "the removal was refused, so it has to still be there for this test to mean anything"
    );

    siege.drive.of(siege.anna).check().await.expect("a look into the Drive");

    assert!(
        siege.archives_in_the_drive().is_empty(),
        "an archive nobody may trust lives on in her Drive: {:?}",
        siege.archives_in_the_drive()
    );
}

#[tokio::test]
async fn a_google_that_names_no_checksum_leaves_the_backup_standing_but_unconfirmed() {
    let siege = Siege::of(0).await;
    siege.google.name_no_checksum_at_all();

    let stored = siege.send().await.expect("a missing checksum is not a broken upload");

    assert_eq!(stored.md5, None, "a checksum was invented where Google gave none");
    assert_eq!(
        siege.google.times_called("files/get"),
        1,
        "the upload answer carried no checksum and nobody went and asked"
    );
    assert_eq!(
        siege.google.file_of_backup(siege.backup).expect("the archive").bytes,
        siege.whole
    );
}

#[tokio::test]
async fn a_sha256_is_the_one_that_counts_when_google_names_it() {
    let siege = Siege::of(1).await;
    siege.google.name_only_a_sha256();

    let stored = siege.send().await.expect("the strongest checksum Google names is a checksum");

    assert_eq!(
        siege.google.times_called("files/get"),
        0,
        "the upload answer already named one and nobody had to go and ask: {:?}",
        siege.google.calls()
    );
    assert_eq!(
        stored.md5.as_deref(),
        Some(md5_of(&siege.whole).as_str()),
        "what is written down is still the md5 of the archive that left this machine"
    );
    assert_eq!(
        siege.google.file_of_backup(siege.backup).expect("the archive").bytes,
        siege.whole
    );
}

#[tokio::test]
async fn a_mangled_archive_is_caught_by_the_sha256_as_well() {
    let siege = Siege::of(1).await;
    siege.google.name_only_a_sha256();
    siege.google.garble_what_arrives();

    let err = siege.send().await.expect_err("a mangled upload is no backup");

    assert_eq!(err.operation_code(), "drive_checksum_mismatch");
    assert!(err.to_string().contains("(sha256)"), "the sentence names the checksum: {err}");
    assert!(
        siege.archives_in_the_drive().is_empty(),
        "a mangled archive stayed in her Drive: {:?}",
        siege.archives_in_the_drive()
    );
}

#[tokio::test]
async fn a_file_google_denies_having_afterwards_is_no_backup_at_all() {
    let siege = Siege::of(0).await;
    siege.google.finish_without_a_checksum();
    siege.google.turn_away_with_body(
        "files/get",
        9,
        404,
        r#"{"error":{"code":404,"errors":[{"reason":"notFound","domain":"global",
            "message":"File not found."}],"message":"File not found."}}"#,
    );

    let err = siege
        .send()
        .await
        .expect_err("a file id that Google denies having is not a backup, confirmed or not");

    assert_eq!(err.operation_code(), "drive_unconfirmed");
    assert!(
        err.to_string().contains("would not say afterwards what it holds"),
        "the sentence has to name what went wrong: {err}"
    );
}

#[tokio::test]
async fn a_google_that_cannot_be_asked_about_the_file_ends_the_run_and_writes_nothing_down() {
    let siege = Siege::of(0).await;
    siege.google.finish_without_a_checksum();
    siege.google.turn_away("files/get", 9, 503, None);

    let err = siege.send().await.expect_err("an unanswered files.get is a failure, not a shrug");

    assert_eq!(err.operation_code(), "drive_unconfirmed");
}

#[tokio::test]
async fn a_restart_sends_the_rest_and_never_the_whole_archive_a_second_time() {
    let siege = Siege::of(2).await;
    siege.stop_after(2).await;
    let offered_by_then = siege.google.bytes_offered();
    assert!(offered_by_then >= super::upload::CHUNK, "nothing was sent before the restart");

    let stored = siege.send_with(&siege.after_a_restart()).await.expect("the rest of the upload");

    let file = siege.google.file_of_backup(siege.backup).expect("the archive in her Drive");
    assert_eq!(file.bytes, siege.whole);
    assert_eq!(stored.md5.as_deref(), Some(md5_of(&siege.whole).as_str()));
    assert!(
        siege.google.bytes_offered() < siege.whole.len() as u64 + super::upload::CHUNK,
        "{} bytes went over the wire for an archive of {} bytes",
        siege.google.bytes_offered(),
        siege.whole.len()
    );
    assert_eq!(
        siege.progress.bytes(),
        siege.whole.len() as u64,
        "the bar counts something other than the archive"
    );
}

#[tokio::test]
async fn an_archive_swapped_for_one_of_the_very_same_size_and_age_never_becomes_one_backup() {
    let siege = Siege::of(2).await;
    siege.stop_after(2).await;
    let before = super::store::print_of(&siege.archive).await.expect("the archive");

    let other = filler(siege.whole.len(), 7);
    assert_ne!(other, siege.whole, "the two worlds have to differ");
    swap_in_place(&siege.archive, &other);
    assert_eq!(
        super::store::print_of(&siege.archive).await.expect("the archive"),
        before,
        "the trap only means something if the archive looks untouched"
    );

    let stored = siege
        .send_with(&siege.after_a_restart())
        .await
        .expect("the archive that is on the disk goes up whole, from the front");

    let file = siege.google.file_of_backup(siege.backup).expect("the archive in her Drive");
    assert_eq!(file.bytes, other, "what lies in the Drive is the archive that lies on the disk");
    assert_eq!(stored.md5.as_deref(), Some(md5_of(&other).as_str()));
    assert_eq!(
        siege.archives_in_the_drive().len(),
        1,
        "the abandoned session left a second file behind: {:?}",
        siege.archives_in_the_drive()
    );
}

#[tokio::test]
async fn a_swapped_archive_is_caught_even_when_google_names_no_checksum_at_all() {
    let siege = Siege::of(2).await;
    siege.stop_after(2).await;
    let before = super::store::print_of(&siege.archive).await.expect("the archive");

    let other = filler(siege.whole.len(), 7);
    swap_in_place(&siege.archive, &other);
    assert_eq!(super::store::print_of(&siege.archive).await.expect("the archive"), before);
    siege.google.name_no_checksum_at_all();

    let stored = siege
        .send_with(&siege.after_a_restart())
        .await
        .expect("a missing checksum is not a broken upload");

    let file = siege.google.file_of_backup(siege.backup).expect("the archive in her Drive");
    assert_eq!(
        file.bytes, other,
        "the half that was already up was spliced onto the archive on the disk"
    );
    assert_ne!(file.bytes, siege.whole, "and it is not the first archive either");
    assert_eq!(stored.md5, None, "Google named nothing, so nothing may say it was confirmed");
}

#[tokio::test]
async fn a_session_google_forgets_in_mid_flight_is_opened_again_in_the_same_run() {
    let siege = Siege::of(2).await;
    siege.google.forget_the_first_session_after(1);

    let stored = siege
        .send()
        .await
        .expect("guides/manage-uploads: a 404 means start over, and that is this run's job");

    assert_eq!(
        siege.google.file_of_backup(siege.backup).expect("the archive").bytes,
        siege.whole
    );
    assert_eq!(stored.md5.as_deref(), Some(md5_of(&siege.whole).as_str()));
    assert_eq!(
        siege.google.times_called("upload/begin"),
        2,
        "the run gave up instead of opening a session of its own: {:?}",
        siege.google.calls()
    );
    assert!(siege.session().await.is_none(), "the dead session was kept");
    assert!(!siege.address().exists());
    assert_eq!(siege.archives_in_the_drive().len(), 1);
}

#[tokio::test]
async fn a_google_that_forgets_every_session_ends_the_run_saying_the_session_ran_out() {
    let siege = Siege::of(2).await;
    siege.google.forget_the_session_after(1);

    let err = siege.send().await.expect_err("Google forgets whatever it is handed");

    assert!(matches!(err, DriveError::SessionOver), "{err:?}");
    assert_eq!(
        err.operation_code(),
        "drive_session_expired",
        "an expired session read as a missing file"
    );
    assert_eq!(
        siege.google.times_called("upload/begin"),
        2,
        "a second session is opened once and not for ever"
    );
    assert!(siege.session().await.is_none(), "the dead session was kept");
    assert!(!siege.address().exists());
    assert!(siege.archives_in_the_drive().is_empty());
}

const NO_CREDENTIALS: &str = r#"{"error":{"code":401,"errors":[{"reason":"authError",
    "domain":"global","message":"Invalid Credentials","locationType":"header",
    "location":"Authorization"}],"message":"Invalid Credentials"}}"#;

#[tokio::test]
async fn a_chunk_that_comes_back_401_is_a_stale_token_and_not_a_withdrawn_connection() {
    let siege = Siege::of(1).await;
    siege.google.fail_chunk(2, 401);

    let stored = siege.send().await.expect("a stale token is renewed and the run carries on");

    assert_eq!(
        siege.google.file_of_backup(siege.backup).expect("the archive").bytes,
        siege.whole,
        "the archive that survived a 401 is not the one on the disk"
    );
    assert_eq!(stored.md5.as_deref(), Some(md5_of(&siege.whole).as_str()));
    assert_eq!(
        siege.google.times_called("token/refresh"),
        2,
        "the one before the run and the one the 401 asked for: {:?}",
        siege.google.calls()
    );
    let status = siege.drive.of(siege.anna).status().await.expect("a status");
    assert_eq!(status.state, Some(DriveAccountState::Connected));
    assert_eq!(status.last_error, None, "an owner was told to reconnect over a stale token");
}

#[tokio::test]
async fn a_401_that_survives_a_fresh_token_is_not_dressed_up_as_a_withdrawal() {
    let siege = Siege::of(1).await;
    siege.google.turn_away_with_body("upload/chunk", 99, 401, NO_CREDENTIALS);

    let err = siege.send().await.expect_err("Google will not take this token at all");

    assert!(
        !err.is_revoked(),
        "a token minted a moment ago is not a connection the owner withdrew: {err:?}"
    );
    assert!(matches!(err, DriveError::Refused { status: 401, .. }), "{err:?}");
    assert_eq!(
        siege.google.times_called("token/refresh"),
        2,
        "the token was renewed over and over instead of once: {:?}",
        siege.google.calls()
    );
    let status = siege.drive.of(siege.anna).status().await.expect("a status");
    assert_eq!(
        status.state,
        Some(DriveAccountState::Connected),
        "a refresh token that still works is not a withdrawn connection"
    );
}

#[tokio::test]
async fn a_token_that_dies_mid_upload_is_renewed_before_the_next_chunk_and_not_after_a_401() {
    let siege = Siege::of(2).await;
    siege.google.let_the_token_die_in(30);

    siege.send().await.expect("an upload longer than one token is still a backup");

    assert_eq!(
        siege.google.file_of_backup(siege.backup).expect("the archive").bytes,
        siege.whole
    );
    assert!(
        siege.google.times_called("token/refresh") >= 4,
        "one token was stretched over a whole upload: {:?}",
        siege.google.calls()
    );
    assert_eq!(
        siege.drive.of(siege.anna).status().await.expect("a status").state,
        Some(DriveAccountState::Connected)
    );
}

#[tokio::test]
async fn two_uploads_at_once_do_not_pull_the_token_out_from_under_each_other() {
    let siege = Siege::of(1).await;
    let (other, elsewhere, tuesday) = siege.a_second_backup().await;
    siege.google.turn_the_first_chunk_of_each_session_away(401, NO_CREDENTIALS);

    let monday = {
        let drive = Arc::clone(&siege.drive);
        let (server, backup, archive, size) =
            (siege.server, siege.backup, siege.archive.clone(), siege.whole.len() as u64);
        tokio::spawn(async move {
            let progress = Arc::new(Progress::default());
            drive
                .upload_archive(server, backup, &archive, size, "monday.tar.zst", &progress)
                .await
        })
    };
    let tuesday_run = {
        let drive = Arc::clone(&siege.drive);
        let (server, size) = (siege.server, tuesday.len() as u64);
        tokio::spawn(async move {
            let progress = Arc::new(Progress::default());
            drive
                .upload_archive(server, other, &elsewhere, size, "tuesday.tar.zst", &progress)
                .await
        })
    };

    monday.await.expect("the task").expect("Monday's archive");
    tuesday_run.await.expect("the task").expect("Tuesday's archive");

    assert_eq!(
        siege.google.times_called("token/refresh"),
        2,
        "one token for the two runs and one renewal between them, no more: {:?}",
        siege.google.calls()
    );
    assert_eq!(
        siege.google.file_of_backup(siege.backup).expect("Monday").bytes,
        siege.whole
    );
    assert_eq!(siege.google.file_of_backup(other).expect("Tuesday").bytes, tuesday);
}

#[tokio::test]
async fn a_connection_withdrawn_before_the_upload_is_written_where_its_owner_can_see_it() {
    let siege = Siege::of(0).await;
    siege.google.withdraw_the_connection();

    let fresh = siege.after_a_restart();
    let err = siege.send_with(&fresh).await.expect_err("the connection is gone");

    assert!(err.is_revoked(), "{err:?}");
    let status = fresh.of(siege.anna).status().await.expect("a status");
    assert_eq!(status.state, Some(DriveAccountState::Revoked));
    assert!(status.last_error.is_some(), "the owner is told nothing");
    assert_eq!(siege.google.times_called("upload/begin"), 0, "an upload without a token");
}

#[tokio::test]
async fn a_google_that_acknowledges_nothing_is_dropped_instead_of_being_fed_for_ever() {
    let siege = Siege::of(1).await;
    siege.google.acknowledge_nothing_ever();

    let started = Instant::now();
    let err = siege.send().await.expect_err("a Drive that swallows everything is no Drive");
    let waited = started.elapsed();

    assert!(matches!(err, DriveError::Unreadable(_)), "{err:?}");
    assert!(waited < Duration::from_secs(30), "the run fed Google for {waited:?}");
    assert!(
        siege.google.bytes_offered() < siege.whole.len() as u64 * 8,
        "{} bytes were poured into a Google that acknowledged none of them",
        siege.google.bytes_offered()
    );
}

#[tokio::test]
async fn a_google_that_claims_more_bytes_than_it_was_given_is_not_believed() {
    let siege = Siege::of(2).await;
    siege.google.claim_more_than_arrived(4096);

    let err = siege.send().await.expect_err("a hole in the file is not a backup");

    assert!(matches!(err, DriveError::Unreadable(_)), "{err:?}");
    assert!(siege.archives_in_the_drive().is_empty());
}

#[tokio::test]
async fn a_file_google_calls_finished_too_early_is_caught_by_its_size() {
    let siege = Siege::of(2).await;
    siege.google.call_it_finished_after(1);

    let err = siege.send().await.expect_err("a third of an archive is not an archive");

    assert_eq!(err.operation_code(), "drive_checksum_mismatch");
    assert!(
        siege.archives_in_the_drive().is_empty(),
        "a short archive was left behind as though it were a backup: {:?}",
        siege.archives_in_the_drive()
    );
}

#[tokio::test]
async fn a_file_google_calls_finished_early_and_names_no_size_for_is_caught_all_the_same() {
    let siege = Siege::of(2).await;
    siege.google.call_it_finished_after(1);
    siege.google.name_no_size_either();

    let err = siege.send().await.expect_err("a third of an archive is not an archive");

    assert_eq!(err.operation_code(), "drive_checksum_mismatch");
    assert!(
        err.to_string().contains("finished after"),
        "the checksum of a third matches the third that was sent, so the length has to be \
         counted on this side: {err}"
    );
    assert!(
        siege.archives_in_the_drive().is_empty(),
        "a short archive was left behind as though it were a backup: {:?}",
        siege.archives_in_the_drive()
    );
}

#[tokio::test]
async fn a_rate_limit_that_is_really_a_dead_session_still_ends_in_a_clean_start() {
    let siege = Siege::of(2).await;
    siege.google.turn_away_with_body("upload/chunk", 1, 403, USER_RATE_LIMIT);
    siege.google.forget_the_first_session_after(1);

    siege.send().await.expect("the rate limit is waited out and the dead session begun again");

    assert_eq!(
        siege.google.file_of_backup(siege.backup).expect("the archive").bytes,
        siege.whole
    );
    assert!(siege.session().await.is_none());
    assert!(!siege.address().exists());
}

#[tokio::test]
async fn the_daily_ceiling_reads_as_a_bad_moment_and_the_owner_is_told_to_try_again_shortly() {
    let siege = Siege::of(0).await;
    siege.google.turn_away_with_body("upload/chunk", 999, 403, USER_RATE_LIMIT);

    let err = siege.send().await.expect_err("the day's bytes are spent");

    assert!(matches!(err, DriveError::RateLimited), "{err:?}");
    assert_eq!(err.operation_code(), "drive_unavailable");
    assert!(
        err.to_string().contains("for the moment"),
        "a ceiling that lasts a day is announced as a moment: {err}"
    );
    assert_eq!(
        siege.google.times_called("upload/chunk"),
        super::retry::Backoff::HURRIED.tries as usize
    );
}

#[tokio::test]
async fn a_session_that_outlived_googles_week_is_not_carried_on() {
    let siege = Siege::of(2).await;
    siege.stop_after(2).await;
    assert!(siege.session().await.is_some());

    let old = Timestamp::at(Timestamp::now().as_datetime() - SESSION_LIFE - time::Duration::hours(1));
    sqlx::query("UPDATE drive_uploads SET opened_at = ? WHERE backup_id = ?")
        .bind(old)
        .bind(siege.backup)
        .execute(&siege.pool)
        .await
        .expect("an old session");

    let sent_by_then = siege.google.bytes_offered();
    siege.send_with(&siege.after_a_restart()).await.expect("a fresh session and a fresh upload");

    assert_eq!(
        siege.google.file_of_backup(siege.backup).expect("the archive").bytes,
        siege.whole
    );
    assert!(
        siege.google.bytes_offered() >= sent_by_then + siege.whole.len() as u64,
        "a session past the week was carried on instead of begun again"
    );
}

fn swap_in_place(path: &std::path::Path, bytes: &[u8]) {
    use std::io::Write;

    let seen = std::fs::metadata(path).expect("the archive");
    let times = std::fs::FileTimes::new()
        .set_accessed(seen.accessed().expect("an access time"))
        .set_modified(seen.modified().expect("a modification time"));
    let mut file = std::fs::File::options().write(true).open(path).expect("the archive");
    file.write_all(bytes).expect("the other archive");
    file.set_times(times).expect("the age it had before");
}

#[tokio::test]
async fn a_download_that_breaks_off_halfway_carries_on_where_it_stopped() {
    let siege = Siege::of(0).await;
    let file = siege.google.put_orphan(&siege.backup.to_string());
    let whole = siege.google.files().into_iter().find(|held| held.id == file).expect("the file");
    siege.google.cut_the_next_download_in_half();
    let into = siege.dir.path().join("back-again.tar.zst");

    let err = siege
        .drive
        .fetch_archive(siege.server, &file, &into, &siege.progress, Recorded::default(), false)
        .await
        .expect_err("half an archive is not the archive");

    assert!(matches!(err, DriveError::Unreachable(_)), "{err:?}");
    let half = std::fs::metadata(&into).expect("the half that arrived").len();
    assert_eq!(half, (whole.bytes.len() / 2) as u64, "the half that arrived was thrown away");

    let brought = siege
        .drive
        .fetch_archive(siege.server, &file, &into, &siege.progress, Recorded::default(), false)
        .await
        .expect("the rest comes down after the half");

    assert_eq!(brought, whole.bytes.len() as u64);
    assert_eq!(std::fs::read(&into).expect("the archive"), whole.bytes);
    assert_eq!(
        siege.google.ranges_asked_for(),
        vec![None, Some(format!("bytes={half}-"))],
        "the second attempt asked for the whole file again"
    );
    assert!(
        !siege.dir.path().join("back-again.tar.zst.source").exists(),
        "the note beside a finished download stays behind"
    );
}

#[tokio::test]
async fn half_a_download_of_another_file_is_never_glued_to_this_one() {
    let siege = Siege::of(0).await;
    let file = siege.google.put_orphan(&siege.backup.to_string());
    let whole = siege.google.files().into_iter().find(|held| held.id == file).expect("the file");
    let into = siege.dir.path().join("back-again.tar.zst");
    std::fs::write(&into, filler(whole.bytes.len() / 2, 9)).expect("half of something else");
    std::fs::write(
        siege.dir.path().join("back-again.tar.zst.source"),
        "some-other-file 12345 0123456789abcdef0123456789abcdef",
    )
    .expect("a note about another file");

    let brought = siege
        .drive
        .fetch_archive(siege.server, &file, &into, &siege.progress, Recorded::default(), false)
        .await
        .expect("the archive comes down");

    assert_eq!(brought, whole.bytes.len() as u64);
    assert_eq!(std::fs::read(&into).expect("the archive"), whole.bytes);
    assert!(
        siege.google.ranges_asked_for().iter().all(Option::is_none),
        "a stranger's half was carried on: {:?}",
        siege.google.ranges_asked_for()
    );
}

#[tokio::test]
async fn a_file_google_calls_abusive_is_only_fetched_after_a_person_has_said_yes() {
    let siege = Siege::of(0).await;
    let file = siege.google.put_orphan(&siege.backup.to_string());
    let whole = siege.google.files().into_iter().find(|held| held.id == file).expect("the file");
    siege.google.call_the_file_abusive();
    let into = siege.dir.path().join("back-again.tar.zst");

    let err = siege
        .drive
        .fetch_archive(siege.server, &file, &into, &siege.progress, Recorded::default(), false)
        .await
        .expect_err("Google will not hand it back unasked");

    assert_eq!(err.operation_code(), "drive_abuse_blocked");
    assert!(err.to_string().contains("malware"), "{err}");
    assert_eq!(
        siege.google.acknowledgements(),
        0,
        "the panel owned up to the risk without ever asking a person"
    );
    assert_eq!(
        siege.google.times_called("files/download"),
        1,
        "an answer only a person can undo was asked again anyway"
    );

    let brought = siege
        .drive
        .fetch_archive(siege.server, &file, &into, &siege.progress, Recorded::default(), true)
        .await
        .expect("the owner said yes");

    assert_eq!(brought, whole.bytes.len() as u64);
    assert_eq!(siege.google.acknowledgements(), 1);
}

#[tokio::test]
async fn a_google_that_says_nothing_at_all_about_the_file_is_never_taken_at_its_word() {
    let siege = Siege::of(0).await;
    siege.google.name_no_checksum_at_all();
    siege.google.name_no_size_either();
    siege.google.turn_away_with_body(
        "files/get",
        9,
        404,
        r#"{"error":{"code":404,"errors":[{"reason":"notFound","domain":"global",
            "message":"File not found."}],"message":"File not found."}}"#,
    );

    let err = siege.send().await.expect_err(
        "neither a size nor a checksum nor an answer to files.get, and it counted as a backup",
    );

    assert_eq!(err.operation_code(), "drive_unconfirmed");
}

#[tokio::test]
async fn a_few_bad_moments_spread_over_the_chunks_are_still_ridden_out() {
    let siege = Siege::of(2).await;
    siege.google.turn_away_every_chunk(3, 429);

    siege.send().await.expect("three bad moments a chunk are still bad moments");

    assert_eq!(
        siege.google.times_called("upload/chunk"),
        12,
        "three chunks, each turned away three times and sent on the fourth: {:?}",
        siege.google.calls()
    );
    assert_eq!(
        siege.google.file_of_backup(siege.backup).expect("the archive").bytes,
        siege.whole
    );
}

#[tokio::test]
async fn the_budget_for_the_waiting_covers_the_run_and_not_only_the_single_call() {
    let siege = Siege::of(2).await;
    siege.google.turn_away("upload/chunk", 999, 429, Some("1"));

    let started = Instant::now();
    let err = siege.send().await.expect_err("a run that does nothing but wait is not a backup");
    let waited = started.elapsed();

    assert!(matches!(err, DriveError::Throttled(_)), "{err:?}");
    assert_eq!(err.operation_code(), "drive_throttled");
    assert!(!err.is_worth_repeating(), "a throttled run is not thrown at Google again at once");
    assert!(
        err.to_string().contains("asked for 1 second"),
        "the sentence does not say when it is worth trying again: {err}"
    );
    assert!(
        waited < super::retry::Pace::HURRIED.run * 4,
        "the run waited {waited:?} on a budget of {:?}",
        super::retry::Pace::HURRIED.run
    );
    assert!(
        siege.google.times_called("upload/chunk") < 12,
        "every chunk was given a budget of its own again: {:?}",
        siege.google.calls()
    );
    assert!(siege.session().await.is_some(), "the half-sent upload is worth carrying on");

    let status = siege.drive.of(siege.anna).status().await.expect("a status");
    assert!(
        status.last_error.is_some_and(|why| why.contains("again and again")),
        "the owner is left to guess why the backup stopped"
    );
}

#[tokio::test]
async fn an_account_that_has_spent_its_day_at_google_never_opens_a_session() {
    let siege = Siege::of(1).await;
    let now = Timestamp::now();
    super::store::note_sent(
        &siege.pool,
        siege.anna,
        &super::day::day_of(now),
        super::day::CEILING,
        now,
    )
    .await
    .expect("a day that is spent");

    let err = siege.send().await.expect_err("Google takes nothing more from this account today");

    assert_eq!(err.operation_code(), "drive_day_full");
    assert!(!err.is_worth_repeating(), "a day's ceiling is not waited out inside one run");
    assert_eq!(
        siege.google.times_called("upload/begin"),
        0,
        "a session was opened into a wall: {:?}",
        siege.google.calls()
    );
    assert_eq!(siege.google.times_called("upload/chunk"), 0);

    let status = siege.drive.of(siege.anna).status().await.expect("a status");
    assert_eq!(status.uploaded_today_bytes, super::day::CEILING);
    assert_eq!(status.daily_upload_limit_bytes, super::day::CEILING);
    assert!(
        status.last_error.is_some_and(|why| why.contains("750 GB")),
        "the owner cannot see why nothing goes up any more"
    );
}

#[tokio::test]
async fn what_went_up_today_is_counted_and_the_count_is_shown() {
    let siege = Siege::of(1).await;

    siege.send().await.expect("an ordinary upload");

    let status = siege.drive.of(siege.anna).status().await.expect("a status");
    assert_eq!(
        status.uploaded_today_bytes,
        siege.whole.len() as u64,
        "an upload that Google took is not counted against the day"
    );
    assert_eq!(
        super::store::sent_today(
            &siege.pool,
            siege.anna,
            &super::day::day_of(Timestamp::now())
        )
        .await
        .expect("the day's row"),
        siege.whole.len() as u64
    );
}

#[tokio::test]
async fn a_drive_with_less_room_than_the_archive_is_told_so_before_a_byte_leaves() {
    let siege = Siege::of(1).await;
    siege.google.leave_room_for(1024);

    let err = siege.send().await.expect_err("it does not fit and no waiting makes it fit");

    assert_eq!(err.operation_code(), "drive_quota_exceeded");
    assert!(!err.is_worth_repeating());
    assert_eq!(
        siege.google.times_called("upload/begin"),
        0,
        "a session was opened for an archive that cannot fit: {:?}",
        siege.google.calls()
    );
    assert_eq!(siege.google.times_called("upload/chunk"), 0);
    assert!(siege.session().await.is_none(), "a session row without a session at Google");
    assert!(siege.archives_in_the_drive().is_empty());
}

#[tokio::test]
async fn a_drive_that_names_no_limit_is_a_case_and_not_a_refusal() {
    let siege = Siege::of(0).await;
    siege.google.name_no_storage_limit();

    siege.send().await.expect("a Workspace account without a limit is not a full Drive");

    assert_eq!(
        siege.google.file_of_backup(siege.backup).expect("the archive").bytes,
        siege.whole
    );
    let status = siege.drive.of(siege.anna).status().await.expect("a status");
    assert_eq!(status.storage_limit_bytes, None, "a missing limit was read as no room at all");
    assert_eq!(status.storage_usage_bytes, Some(2_147_483_648));
}

const FILE_NOT_FOUND: &str = r#"{"error":{"code":404,"errors":[{"reason":"notFound",
    "domain":"global","message":"File not found."}],"message":"File not found."}}"#;

const BACKEND_ERROR: &str = r#"{"error":{"code":500,"errors":[{"reason":"backendError",
    "domain":"global","message":"Backend Error"}],"message":"Backend Error"}}"#;

impl Siege {
    fn a_file_in_the_drive(&self, bytes: &[u8]) -> String {
        let id = self.google.put_orphan(&self.backup.to_string());
        self.google.swap_the_file(&id, bytes);
        id
    }

    fn landing(&self) -> std::path::PathBuf {
        self.dir.path().join("back-again.tar.zst")
    }

    fn mark(&self) -> std::path::PathBuf {
        self.dir.path().join("back-again.tar.zst.source")
    }

    async fn bring_back(
        &self,
        file: &str,
        into: &std::path::Path,
        warned: bool,
    ) -> std::result::Result<u64, DriveError> {
        self.bring_back_knowing(file, into, Recorded::default(), warned).await
    }

    async fn bring_back_knowing(
        &self,
        file: &str,
        into: &std::path::Path,
        recorded: Recorded<'_>,
        warned: bool,
    ) -> std::result::Result<u64, DriveError> {
        let progress = Progress::default();
        self.drive.fetch_archive(self.server, file, into, &progress, recorded, warned).await
    }
}

#[tokio::test]
async fn an_untouched_archive_is_still_carried_on_when_google_names_no_checksum() {
    let siege = Siege::of(2).await;
    siege.stop_after(2).await;
    siege.google.name_no_checksum_at_all();
    let offered_by_then = siege.google.bytes_offered();
    assert!(offered_by_then >= super::upload::CHUNK, "nothing was sent before the restart");

    let stored = siege
        .send_with(&siege.after_a_restart())
        .await
        .expect("the archive on the disk is the one the session was fed");

    let file = siege.google.file_of_backup(siege.backup).expect("the archive in her Drive");
    assert_eq!(file.bytes, siege.whole);
    assert_eq!(stored.md5, None, "Google named nothing, so nothing may say it was confirmed");
    assert!(
        siege.google.bytes_offered() - offered_by_then < siege.whole.len() as u64,
        "the whole archive went up a second time although not a byte of it had changed: \
         {} more bytes for an archive of {}",
        siege.google.bytes_offered() - offered_by_then,
        siege.whole.len()
    );
}

#[tokio::test]
async fn an_archive_rewritten_under_a_running_upload_never_becomes_one_backup() {
    let siege = Siege::of(2).await;
    siege.google.hold_the_chunk(1, Duration::from_millis(600));
    let other = filler(siege.whole.len(), 11);
    assert_ne!(other, siege.whole, "the two worlds have to differ");

    let sending = {
        let drive = Arc::clone(&siege.drive);
        let (server, backup, archive, size) =
            (siege.server, siege.backup, siege.archive.clone(), siege.whole.len() as u64);
        tokio::spawn(async move {
            let progress = Arc::new(Progress::default());
            drive.upload_archive(server, backup, &archive, size, "monday.tar.zst", &progress).await
        })
    };
    for _ in 0..400 {
        if siege.google.chunks_seen() >= 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(siege.google.chunks_seen() >= 1, "the first chunk never reached Google");
    let before = super::store::print_of(&siege.archive).await.expect("the archive");
    rewrite_in_place(&siege.archive, &other);
    let after = super::store::print_of(&siege.archive).await.expect("the archive");
    assert_eq!(after.inode, before.inode, "the archive has to be the same file on the disk");
    assert_eq!(after.bytes, before.bytes, "and it has to be the same length");

    let err = sending.await.expect("the task").expect_err(
        "the front of one archive and the back of another counted as a backup",
    );

    assert_eq!(err.operation_code(), "drive_checksum_mismatch", "{err:?}");
    assert!(
        err.to_string().contains("written over while it was going up"),
        "the sentence does not say what happened to the archive: {err}"
    );
    assert!(
        siege.archives_in_the_drive().is_empty(),
        "the spliced archive was left in the Drive to be restored one day: {:?}",
        siege.archives_in_the_drive()
    );
}

fn rewrite_in_place(path: &std::path::Path, bytes: &[u8]) {
    use std::io::Write;

    let mut file = std::fs::File::options().write(true).open(path).expect("the archive");
    file.write_all(bytes).expect("the other archive");
    file.sync_all().expect("the other archive on the disk");
}

#[tokio::test]
async fn a_files_get_that_answers_404_stops_the_restore_before_a_byte_is_written() {
    let siege = Siege::of(0).await;
    let file = siege.a_file_in_the_drive(&filler(64 * 1024, 3));
    siege.google.turn_away_with_body("files/get", 9, 404, FILE_NOT_FOUND);

    let err = siege
        .bring_back(&file, &siege.landing(), false)
        .await
        .expect_err("a file Google denies having cannot come down");

    assert_eq!(err.operation_code(), "drive_file_missing", "{err:?}");
    assert!(!err.is_worth_repeating(), "a file that is not there does not appear by asking again");
    assert_eq!(
        siege.google.times_called("files/get"),
        1,
        "a 404 was asked again and again: {:?}",
        siege.google.calls()
    );
    assert_eq!(siege.google.times_called("files/download"), 0, "a byte was asked for anyway");
    assert!(!siege.landing().exists(), "half a restore was left on the disk");
    assert!(!siege.mark().exists(), "a note was left beside a file that never arrived");
}

#[tokio::test]
async fn a_files_get_that_never_answers_holds_the_restore_for_a_bounded_while_only() {
    let siege = Siege::of(0).await;
    let file = siege.a_file_in_the_drive(&filler(64 * 1024, 3));
    siege.google.turn_away_with_body("files/get", 999, 500, BACKEND_ERROR);

    let started = Instant::now();
    let err = siege
        .bring_back(&file, &siege.landing(), false)
        .await
        .expect_err("a Google that only says 500 hands nothing back");
    let waited = started.elapsed();

    assert!(matches!(err, DriveError::Refused { status: 500, .. }), "{err:?}");
    assert_eq!(
        siege.google.times_called("files/get"),
        super::retry::Backoff::HURRIED.tries as usize,
        "the ceiling on the tries was not held to: {:?}",
        siege.google.calls()
    );
    assert!(
        waited < super::retry::Backoff::HURRIED.budget * 3,
        "the restore hung for {waited:?} on a budget of {:?}",
        super::retry::Backoff::HURRIED.budget
    );
    assert_eq!(siege.google.times_called("files/download"), 0);
    assert!(!siege.landing().exists());
    assert!(!siege.mark().exists());
}

#[tokio::test]
async fn a_connection_withdrawn_in_the_middle_of_an_upload_stands_in_the_owners_status_line() {
    let siege = Siege::of(2).await;
    siege.google.fail_chunk(2, 401);
    siege.google.withdraw_the_connection();

    let err = siege.send().await.expect_err("the connection was taken away mid-flight");

    assert!(err.is_revoked(), "{err:?}");
    assert_eq!(err.operation_code(), "drive_revoked");
    let status = siege.drive.of(siege.anna).status().await.expect("a status");
    assert_eq!(
        status.state,
        Some(DriveAccountState::Revoked),
        "the account still reads as connected after Google let it go"
    );
    assert!(status.last_error.is_some(), "the owner is left to guess why the backup stopped");
    assert!(
        siege.archives_in_the_drive().is_empty(),
        "half an archive was left behind: {:?}",
        siege.archives_in_the_drive()
    );
}

#[tokio::test]
async fn nine_refusals_that_each_ask_for_a_second_cost_the_run_its_budget_and_no_more() {
    let siege = Siege::of(2).await;
    siege.google.turn_away("upload/chunk", 9, 429, Some("1"));

    let started = Instant::now();
    let err = siege.send().await.expect_err("a run that does nothing but wait is not a backup");
    let waited = started.elapsed();

    assert!(matches!(err, DriveError::Throttled(_)), "{err:?}");
    assert!(!err.is_worth_repeating());
    assert!(
        waited < super::retry::Pace::HURRIED.run * 3,
        "the run waited {waited:?} on a budget of {:?}",
        super::retry::Pace::HURRIED.run
    );
    assert!(
        siege.google.times_called("upload/chunk") <= 4,
        "the budget was handed out again for every chunk: {:?}",
        siege.google.calls()
    );
    assert!(siege.session().await.is_some(), "the half-sent upload is worth carrying on");
    assert!(siege.archives_in_the_drive().is_empty());
}

#[tokio::test]
async fn a_download_broken_off_at_thirty_and_fifty_and_ninety_nine_percent_still_comes_down_whole()
{
    let siege = Siege::of(0).await;
    let whole = filler(200_000, 5);
    let file = siege.a_file_in_the_drive(&whole);
    siege.google.cut_the_downloads_at(&[30, 50, 99]);
    let into = siege.landing();

    for at in [30usize, 50, 99] {
        let err = siege
            .bring_back(&file, &into, false)
            .await
            .expect_err("a part of an archive is not the archive");
        assert!(matches!(err, DriveError::Unreachable(_)), "at {at}%: {err:?}");
        assert_eq!(
            std::fs::metadata(&into).expect("the part that arrived").len(),
            (whole.len() * at / 100) as u64,
            "the part that arrived at {at}% was thrown away"
        );
    }

    let brought = siege.bring_back(&file, &into, false).await.expect("the last of it comes down");

    assert_eq!(brought, whole.len() as u64);
    assert_eq!(std::fs::read(&into).expect("the archive"), whole, "byte for byte, it is not it");
    assert_eq!(
        siege.google.ranges_asked_for(),
        vec![
            None,
            Some("bytes=60000-".to_owned()),
            Some("bytes=100000-".to_owned()),
            Some("bytes=198000-".to_owned()),
        ],
        "an attempt asked for more of the archive than it was missing"
    );
    assert_eq!(
        siege.google.bytes_handed_out(),
        whole.len() as u64,
        "an archive of {} bytes cost {} bytes over the wire",
        whole.len(),
        siege.google.bytes_handed_out()
    );
    assert!(!siege.mark().exists(), "the note beside a finished download stays behind");
}

#[tokio::test]
async fn a_file_swapped_in_the_drive_between_two_halves_of_a_download_never_becomes_one_archive() {
    let siege = Siege::of(0).await;
    let first = filler(120_000, 5);
    let second = filler(120_000, 9);
    assert_ne!(first, second, "the two archives have to differ");
    let file = siege.a_file_in_the_drive(&first);
    siege.google.cut_the_downloads_at(&[50]);
    let into = siege.landing();

    siege.bring_back(&file, &into, false).await.expect_err("half an archive is not the archive");
    assert_eq!(std::fs::metadata(&into).expect("the half").len(), 60_000);
    siege.google.swap_the_file(&file, &second);

    let brought = siege.bring_back(&file, &into, false).await.expect("the other archive comes down");

    assert_eq!(brought, second.len() as u64);
    let landed = std::fs::read(&into).expect("the archive");
    assert_eq!(landed, second, "the half of the first archive was glued to the second");
    assert_ne!(landed, first);
    assert_eq!(
        siege.google.ranges_asked_for(),
        vec![None, None],
        "the panel carried on into a file that is no longer the one it began: {:?}",
        siege.google.ranges_asked_for()
    );
}

#[tokio::test]
async fn the_word_that_lets_malware_through_is_never_sent_unless_a_person_said_it() {
    let siege = Siege::of(0).await;
    let whole = filler(80_000, 4);
    let file = siege.a_file_in_the_drive(&whole);
    let into = siege.landing();
    siege.google.cut_the_downloads_at(&[50]);
    siege.bring_back(&file, &into, false).await.expect_err("half an archive is not the archive");
    assert_eq!(std::fs::metadata(&into).expect("the half").len(), 40_000);

    siege.google.call_the_file_abusive();
    for _ in 0..3 {
        let err = siege
            .bring_back(&file, &into, false)
            .await
            .expect_err("Google will not hand it back unasked");
        assert_eq!(err.operation_code(), "drive_abuse_blocked", "{err:?}");
        assert!(!err.is_worth_repeating());
    }
    assert_eq!(
        siege.google.acknowledgements(),
        0,
        "the panel owned up to the risk without ever asking a person"
    );
    assert_eq!(
        siege.google.times_called("files/download"),
        4,
        "an answer only a person can undo was asked again inside a run: {:?}",
        siege.google.calls()
    );

    let brought = siege.bring_back(&file, &into, true).await.expect("the owner said yes");

    assert_eq!(brought, whole.len() as u64);
    assert_eq!(std::fs::read(&into).expect("the archive"), whole);
    assert_eq!(siege.google.acknowledgements(), 1, "one yes, one call that carries it");

    siege.google.cut_the_downloads_at(&[50]);
    let err = siege
        .bring_back(&file, &into, false)
        .await
        .expect_err("the yes was for that one run and not for ever");
    assert_eq!(err.operation_code(), "drive_abuse_blocked", "{err:?}");
    assert_eq!(siege.google.acknowledgements(), 1);
}

#[tokio::test]
async fn a_file_this_account_does_not_own_is_not_dressed_up_as_a_backup() {
    let siege = Siege::of(0).await;
    let whole = filler(64 * 1024, 8);
    let file = siege.a_file_in_the_drive(&whole);
    siege.google.say_the_file_is_not_ours();
    siege.google.turn_away_with_body("files/download", 1, 403, NO_WRITE_ACCESS);
    let into = siege.landing();

    let err = siege
        .bring_back(&file, &into, false)
        .await
        .expect_err("a file this account may not read is not a backup it can restore");

    assert!(matches!(err, DriveError::Refused { status: 403, .. }), "{err:?}");
    assert!(
        !err.is_worth_repeating(),
        "permission does not appear by asking again: {err:?}"
    );
    assert_eq!(
        siege.google.times_called("files/download"),
        1,
        "a refusal only a person can lift was thrown at Google again: {:?}",
        siege.google.calls()
    );
    assert!(!into.exists(), "half a restore was left on the disk");

    let brought = siege
        .bring_back(&file, &into, false)
        .await
        .expect("a file the panel never opened is still the owner's file");

    assert_eq!(brought, whole.len() as u64);
    assert_eq!(std::fs::read(&into).expect("the archive"), whole);
}

#[tokio::test]
async fn a_disk_that_runs_out_while_the_archive_comes_down_is_not_a_finished_restore() {
    let siege = Siege::of(0).await;
    let whole = filler(256 * 1024, 6);
    let file = siege.a_file_in_the_drive(&whole);
    let into = siege.landing();
    siege.google.hold_the_download(Duration::from_millis(400));

    let full = {
        let into = into.clone();
        tokio::spawn(async move {
            for _ in 0..400 {
                if std::fs::symlink_metadata(&into).is_err() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            std::os::unix::fs::symlink("/dev/full", &into)
        })
    };
    std::fs::write(&into, b"the part of an earlier attempt").expect("something in the way");

    let started = Instant::now();
    let err = siege
        .bring_back(&file, &into, false)
        .await
        .expect_err("a disk with no room on it is not a restore");
    let waited = started.elapsed();

    full.await.expect("the task").expect("a disk with no room on it");
    assert!(matches!(err, DriveError::Unreachable(_)), "{err:?}");
    assert!(
        err.to_string().contains("could not be written to disk"),
        "the sentence does not say the disk would not take it: {err}"
    );
    assert!(
        err.to_string().contains("No space left"),
        "a full disk was reported as something else: {err}"
    );
    assert!(waited < Duration::from_secs(10), "the run held on for {waited:?} over a full disk");
    assert_eq!(
        siege.google.times_called("files/download"),
        1,
        "a full disk was thrown at Google again: {:?}",
        siege.google.calls()
    );

    std::fs::remove_file(&into).expect("the disk with no room on it goes away");
    let brought = siege
        .bring_back(&file, &into, false)
        .await
        .expect("a disk with room again brings the archive down");

    assert_eq!(brought, whole.len() as u64);
    assert_eq!(std::fs::read(&into).expect("the archive"), whole);
}

#[tokio::test]
async fn a_google_that_ignores_the_range_and_sends_everything_again_does_not_double_the_archive() {
    let siege = Siege::of(0).await;
    let whole = filler(120_000, 12);
    let file = siege.a_file_in_the_drive(&whole);
    siege.google.cut_the_downloads_at(&[50]);
    let into = siege.landing();
    siege.bring_back(&file, &into, false).await.expect_err("half an archive is not the archive");
    siege.google.ignore_the_range();

    let brought = siege.bring_back(&file, &into, false).await.expect("the archive comes down");

    assert_eq!(brought, whole.len() as u64);
    assert_eq!(
        std::fs::read(&into).expect("the archive"),
        whole,
        "the half that was already here was kept in front of a whole file"
    );
    assert_eq!(
        siege.google.ranges_asked_for(),
        vec![None, Some("bytes=60000-".to_owned())],
        "{:?}",
        siege.google.ranges_asked_for()
    );
    assert_eq!(
        siege.google.bytes_handed_out(),
        60_000 + whole.len() as u64,
        "a Google that will not resume cost more than the archive twice over"
    );
}

#[tokio::test]
async fn a_206_that_starts_at_the_front_is_never_appended_to_what_is_already_there() {
    let siege = Siege::of(0).await;
    let whole = filler(120_000, 13);
    let file = siege.a_file_in_the_drive(&whole);
    siege.google.cut_the_downloads_at(&[50]);
    let into = siege.landing();
    siege.bring_back(&file, &into, false).await.expect_err("half an archive is not the archive");
    siege.google.answer_from_the_front();

    let err = siege
        .bring_back(&file, &into, false)
        .await
        .expect_err("an archive one and a half times its length is not the archive");

    assert!(matches!(err, DriveError::Unreadable(_)), "{err:?}");
    assert!(!into.exists(), "the archive with a doubled front was left where a restore finds it");
    assert!(!siege.mark().exists());
}

#[tokio::test]
async fn an_archive_google_names_neither_a_size_nor_a_checksum_for_is_not_called_whole() {
    let siege = Siege::of(0).await;
    let whole = filler(120_000, 14);
    let file = siege.a_file_in_the_drive(&whole);
    siege.google.name_no_checksum_at_all();
    siege.google.name_no_size_either();
    siege.google.cut_the_downloads_at(&[40]);
    let into = siege.landing();

    let recorded = Recorded { bytes: Some(whole.len() as u64), md5: None };

    let brought = siege.bring_back_knowing(&file, &into, recorded, false).await;

    let landed = std::fs::metadata(&into).map(|seen| seen.len()).unwrap_or(0);
    let err = brought.expect_err("two fifths of an archive counted as a whole restore");
    assert!(matches!(err, DriveError::Unreachable(_)), "{err:?}");
    assert!(
        err.to_string().contains("48000"),
        "the sentence does not say how much of it arrived: {err}"
    );
    assert_eq!(landed, 48_000, "what did arrive was thrown away");

    let brought = siege
        .bring_back_knowing(&file, &into, recorded, false)
        .await
        .expect("the archive comes down whole on the next attempt");

    assert_eq!(brought, whole.len() as u64);
    assert_eq!(std::fs::read(&into).expect("the archive"), whole);
    assert_eq!(
        siege.google.bytes_handed_out(),
        48_000 + whole.len() as u64,
        "an archive Google names no checksum for cannot be carried on, and that is the price"
    );
}

#[tokio::test]
async fn a_file_google_names_another_size_for_than_the_one_that_went_up_is_never_fetched() {
    let siege = Siege::of(0).await;
    let whole = filler(120_000, 16);
    let file = siege.a_file_in_the_drive(&whole);
    let into = siege.landing();

    let err = siege
        .bring_back_knowing(&file, &into, Recorded { bytes: Some(99_000), md5: None }, false)
        .await
        .expect_err("what lies under that id is not the archive that went up");

    assert!(matches!(err, DriveError::Unreadable(_)), "{err:?}");
    assert!(err.to_string().contains("99000"), "{err}");
    assert_eq!(
        siege.google.times_called("files/download"),
        0,
        "a byte of the wrong file was fetched anyway: {:?}",
        siege.google.calls()
    );
    assert_eq!(siege.google.bytes_handed_out(), 0);
    assert!(!into.exists());
    assert!(!siege.mark().exists());
}

#[tokio::test]
async fn a_file_swapped_while_the_second_half_is_on_the_wire_is_never_glued_together() {
    let siege = Siege::of(0).await;
    let first = filler(120_000, 5);
    let second = filler(120_000, 21);
    assert_ne!(first, second, "the two archives have to differ");
    let file = siege.a_file_in_the_drive(&first);
    siege.google.cut_the_downloads_at(&[50]);
    let into = siege.landing();
    siege.bring_back(&file, &into, false).await.expect_err("half an archive is not the archive");
    siege.google.hold_the_download(Duration::from_millis(400));

    let swapping = {
        let google = &siege.google;
        async {
            for _ in 0..400 {
                if google.times_called("files/download") >= 2 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            google.swap_the_file(&file, &second);
        }
    };
    let (brought, ()) = tokio::join!(siege.bring_back(&file, &into, false), swapping);

    let err = brought.expect_err("the back of one archive on the front of another is no archive");
    assert!(matches!(err, DriveError::Unreadable(_)), "{err:?}");
    assert!(!into.exists(), "the spliced archive was left where a restore finds it");
    assert!(!siege.mark().exists());
    assert_eq!(
        siege.google.ranges_asked_for(),
        vec![None, Some("bytes=60000-".to_owned())],
        "{:?}",
        siege.google.ranges_asked_for()
    );
}

#[tokio::test]
async fn an_archive_in_the_bin_of_the_drive_is_not_fetched_at_all() {
    let siege = Siege::of(0).await;
    let file = siege.a_file_in_the_drive(&filler(64 * 1024, 15));
    siege.google.trash_file(&file);
    let into = siege.landing();

    let err = siege
        .bring_back(&file, &into, false)
        .await
        .expect_err("what the owner put in the bin is not a backup any more");

    assert_eq!(err.operation_code(), "drive_file_missing", "{err:?}");
    assert_eq!(siege.google.times_called("files/download"), 0, "a byte was asked for anyway");
    assert!(!into.exists());
    assert!(!siege.mark().exists());
}

#[tokio::test]
async fn an_archive_swapped_in_the_drive_for_one_of_the_same_length_never_comes_back_as_a_restore()
{
    let siege = Siege::of(0).await;
    let ours = filler(160_000, 31);
    let stranger = filler(160_000, 32);
    assert_ne!(ours, stranger, "the two archives have to differ");
    assert_eq!(ours.len(), stranger.len(), "a swap of another length is caught by the size alone");
    let file = siege.a_file_in_the_drive(&ours);
    let written_down = md5_of(&ours);
    let ours_recorded =
        Recorded { bytes: Some(ours.len() as u64), md5: Some(written_down.as_str()) };
    let into = siege.landing();

    let brought = siege
        .bring_back_knowing(&file, &into, ours_recorded, false)
        .await
        .expect("the archive that lies there is the one that went up");
    assert_eq!(brought, ours.len() as u64);
    assert_eq!(std::fs::read(&into).expect("the archive"), ours);
    std::fs::remove_file(&into).expect("the archive goes away again");

    siege.google.swap_the_file(&file, &stranger);
    let only_the_size = Recorded { bytes: Some(stranger.len() as u64), md5: None };

    let through = siege
        .bring_back_knowing(&file, &into, only_the_size, false)
        .await
        .expect("Google names the new checksum, the download matches it, and the size is right");
    assert_eq!(through, stranger.len() as u64);
    assert_eq!(
        std::fs::read(&into).expect("the archive"),
        stranger,
        "the swap has to be invisible to every check that came before this one, or this attack \
         proves nothing"
    );
    std::fs::remove_file(&into).expect("the stranger goes away again");

    let err = siege
        .bring_back_knowing(&file, &into, ours_recorded, false)
        .await
        .expect_err("a stranger's archive was handed back as this backup");

    assert_eq!(err.operation_code(), "drive_file_replaced", "{err:?}");
    assert!(err.to_string().contains(&written_down), "the sentence names what went up: {err}");
    assert!(
        err.to_string().contains(&md5_of(&stranger)),
        "and it names what lies there now: {err}"
    );
    assert!(
        err.to_string().contains("written over"),
        "a file that was replaced is not a broken line, and must not read like one: {err}"
    );
    assert!(!err.is_worth_repeating(), "asking Google again does not put the archive back");
    assert!(!into.exists(), "the stranger's archive was left where a restore finds it");
    assert!(!siege.mark().exists(), "and a note was left to carry it on with");
}

#[tokio::test]
async fn an_untouched_archive_comes_back_even_when_google_names_another_kind_of_checksum() {
    let siege = Siege::of(0).await;
    let ours = filler(160_000, 33);
    let file = siege.a_file_in_the_drive(&ours);
    let written_down = md5_of(&ours);
    let recorded = Recorded { bytes: Some(ours.len() as u64), md5: Some(written_down.as_str()) };
    let into = siege.landing();
    siege.google.name_only_a_sha256();
    siege.google.cut_the_downloads_at(&[50]);

    siege
        .bring_back_knowing(&file, &into, recorded, false)
        .await
        .expect_err("half an archive is not the archive");

    let brought = siege
        .bring_back_knowing(&file, &into, recorded, false)
        .await
        .expect("what was written down is our own md5 of our own bytes, and no name Google \
                 gives its checksum today can change that");

    assert_eq!(brought, ours.len() as u64);
    assert_eq!(std::fs::read(&into).expect("the archive"), ours, "byte for byte, it is not it");
    assert_eq!(
        siege.google.ranges_asked_for(),
        vec![None, Some("bytes=80000-".to_owned())],
        "the half that was already here was fetched a second time: {:?}",
        siege.google.ranges_asked_for()
    );
    assert!(!siege.mark().exists());
}

#[tokio::test]
async fn the_hourly_look_finds_a_swapped_archive_before_anybody_needs_it() {
    let siege = Siege::of(0).await;
    let stored = siege.send().await.expect("the archive goes up");
    crate::backups::store::finish_upload(
        &siege.pool,
        siege.backup,
        &stored.file_id,
        siege.whole.len() as u64,
        stored.md5.as_deref(),
        Timestamp::now(),
    )
    .await
    .expect("the row a finished upload leaves");

    siege.drive.of(siege.anna).check().await.expect("a look into the Drive");
    let row = crate::backups::store::find(&siege.pool, siege.backup).await.expect("the row");
    assert_eq!(row.drive_content_changed_at, None, "an untouched archive was called changed");

    let stranger = filler(siege.whole.len(), 41);
    siege.google.swap_the_file(&stored.file_id, &stranger);
    siege.drive.of(siege.anna).check().await.expect("a look into the Drive");

    let row = crate::backups::store::find(&siege.pool, siege.backup).await.expect("the row");
    assert!(
        row.drive_content_changed_at.is_some(),
        "the hourly look walked past a file that is no longer the archive it lists"
    );
    assert_eq!(
        row.drive_state,
        Some(DriveFileState::Present),
        "the file is still in the Drive, and saying otherwise sends the owner to look in the bin"
    );
    let seen = crate::backups::store::one(&siege.pool, siege.backup).await.expect("the page row");
    assert_eq!(
        seen.drive_content_changed,
        Some(true),
        "nothing on the page says this backup is no longer the backup"
    );
    assert_eq!(
        seen.drive_verified,
        Some(true),
        "it was confirmed on the way up, and that stays true whatever happened later"
    );
    assert_eq!(
        siege.google.times_called("files/get"),
        0,
        "the sweep paid for a checksum that files.list already carries: {:?}",
        siege.google.calls()
    );

    siege.google.swap_the_file(&stored.file_id, &siege.whole);
    siege.drive.of(siege.anna).check().await.expect("a look into the Drive");

    let row = crate::backups::store::find(&siege.pool, siege.backup).await.expect("the row");
    assert_eq!(
        row.drive_content_changed_at, None,
        "the owner put his own file back and the panel kept the red mark"
    );
}
