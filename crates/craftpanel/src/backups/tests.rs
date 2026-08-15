use std::sync::Arc;

use super::testing::FakeServer;
use super::*;
use crate::model::{BackupOperationState, BackupStatus, OperationKind, OperationState};

#[tokio::test]
async fn a_backup_is_refused_when_the_owners_disk_pot_is_full() {
    const MIB: u64 = 1024 * 1024;
    let game = FakeServer::holding_disk(crate::auth::Disks::fixed(1024 * MIB, 0)).await;
    game.file("world/level.dat", b"a world");
    sqlx::query("UPDATE users SET disk_mib = 1024 WHERE id = ?")
        .bind(game.owner)
        .execute(game.pool())
        .await
        .unwrap();

    let refused = game.backups.request(game.server, "one", game.owner).await.unwrap_err();
    assert_eq!(refused.code(), "disk_limit_reached");
    assert_eq!(refused.status(), axum::http::StatusCode::CONFLICT);

    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM backups")
        .fetch_one(game.pool())
        .await
        .unwrap();
    assert_eq!(rows, 0, "no row for a run that never started");
    assert!(game.server_dir().join("world/level.dat").exists(), "and nothing was cleaned up");

    sqlx::query("UPDATE users SET disk_mib = 8192 WHERE id = ?")
        .bind(game.owner)
        .execute(game.pool())
        .await
        .unwrap();
    assert!(game.backups.request(game.server, "one", game.owner).await.is_ok());
}

#[tokio::test]
async fn an_archive_lies_outside_the_server_directory_and_no_backup_holds_an_older_one() {
    let game = FakeServer::stopped().await;
    game.file("world/level.dat", b"a world");
    game.file("mods/handy.jar", b"a mod");

    let first = game.a_finished_backup("one").await;
    let path = game.backups.archive_of(game.server, first);
    assert!(path.exists(), "the archive is written");
    assert!(
        !path.starts_with(game.server_dir()),
        "{} must not be under {}",
        path.display(),
        game.server_dir().display()
    );
    assert_eq!(path.parent(), Some(game.backups.dir_of(game.server).as_path()));

    let second = game.a_finished_backup("two").await;
    let out = game.data_dir().join("look");
    archive::unpack(
        &game.backups.archive_of(game.server, second),
        &out,
        &archive::Progress::default(),
    )
    .expect("the second archive opens");

    let names: Vec<String> = walkdir::WalkDir::new(&out)
        .into_iter()
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path().display().to_string())
        .collect();
    assert!(names.iter().any(|name| name.ends_with("world/level.dat")));
    assert!(
        !names.iter().any(|name| name.ends_with(".tar.zst")),
        "the second backup packed the first: {names:?}"
    );
}

#[tokio::test]
async fn the_backup_directory_is_readable_by_the_panel_alone() {
    use std::os::unix::fs::PermissionsExt;
    let game = FakeServer::stopped().await;
    game.file("world/level.dat", b"a world");
    game.a_finished_backup("one").await;

    for dir in [game.backups.root(), game.backups.dir_of(game.server)] {
        let mode = std::fs::metadata(&dir).expect("the directory").permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o700,
            "10: the game process runs as craft-<id> and would otherwise reach {}",
            dir.display()
        );
    }
}

#[tokio::test]
async fn a_backup_of_a_running_server_is_bracketed_by_save_off_and_save_on() {
    let game = FakeServer::start().await;
    game.file("world/level.dat", b"a world");

    let backup = game.a_finished_backup("while it runs").await;
    game.settle().await;

    assert_eq!(
        game.commands().await,
        vec!["save-off", "save-all flush", "save-on"],
        "10.2: the world is written asynchronously, so the pack has to sit between these"
    );
    assert_eq!(
        crate::backups::store::one(game.pool(), backup).await.expect("the backup").status,
        BackupStatus::Done
    );
}

#[tokio::test]
async fn a_pack_that_fails_still_switches_saving_back_on() {
    let game = FakeServer::start().await;
    game.file("world/level.dat", b"a world");

    let queued = game
        .backups
        .create(game.server, "doomed", Some(game.owner), false)
        .await
        .expect("a queued backup");
    std::fs::create_dir_all(game.backups.archive_of(game.server, queued.backup))
        .expect("something in the way");

    game.backups.run(queued.operation).await;
    game.settle().await;

    let run = game.operations.get(queued.operation).await.expect("the run");
    assert_eq!(run.state, OperationState::Failed);
    assert_eq!(
        game.commands().await.last().map(String::as_str),
        Some("save-on"),
        "a server left with saving off is the worst thing this area can do"
    );
}

#[tokio::test]
async fn a_stopped_server_is_not_sent_any_save_commands() {
    let game = FakeServer::stopped().await;
    game.file("world/level.dat", b"a world");
    game.a_finished_backup("one").await;
    assert!(game.commands().await.is_empty());
}

#[tokio::test]
async fn restoring_makes_a_safety_copy_first_and_hands_the_files_back_afterwards() {
    let game = FakeServer::stopped().await;
    game.file("world/level.dat", b"the old world");
    game.file("mods/handy.jar", b"a mod");
    let source = game.a_finished_backup("before the accident").await;

    std::fs::remove_dir_all(game.server_dir().join("world")).expect("a lost world");
    std::fs::write(game.server_dir().join("mods/handy.jar"), b"broken").expect("a broken mod");

    let accepted = game
        .backups
        .restore(game.server, source, "safety net", game.owner)
        .await
        .expect("a restore");
    assert_eq!(game.await_operation(accepted.restore_operation_id).await, OperationState::Done);

    assert_eq!(
        std::fs::read(game.server_dir().join("world/level.dat")).expect("the world is back"),
        b"the old world"
    );
    assert_eq!(
        std::fs::read(game.server_dir().join("mods/handy.jar")).expect("the mod"),
        b"a mod",
        "10: the whole server directory comes back, not only the world"
    );

    let list = game.backups.list(game.server).await.expect("a list");
    let safety = list
        .backups
        .iter()
        .find(|backup| backup.id == accepted.safety_backup.id)
        .expect("the safety copy is in the list");
    assert!(!safety.automated, "10.6: a safety copy is not an automatic backup");
    assert_eq!(safety.status, BackupStatus::Done);
    let made = &safety.history[0];
    assert!(made.has_parent, "10.1: without this the interface blocks for the whole restore");
    assert!(!made.should_prompt, "10.6: two success banners side by side otherwise");

    let working = format!("servers/{}.restoring-", game.server);
    assert!(
        game.chowned().iter().any(|steps| steps.starts_with(&working)),
        "docs/PLAN.md:205 — restored files belong to the panel until they are handed back; \
         got {:?}",
        game.chowned()
    );
    assert!(beside_the_server(&game).is_empty(), "and the working names are gone again");
}

fn beside_the_server(game: &FakeServer) -> Vec<std::path::PathBuf> {
    let directory = game.server_dir();
    let parent = directory.parent().expect("a parent").to_owned();
    std::fs::read_dir(parent)
        .expect("the servers directory")
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            (path != directory).then_some(path)
        })
        .collect()
}

#[tokio::test]
async fn a_leftover_that_cannot_be_cleared_does_not_block_the_next_restore() {
    let game = FakeServer::stopped().await;
    game.file("world/level.dat", b"the old world");
    let source = game.a_finished_backup("before the accident").await;
    std::fs::write(game.server_dir().join("world/level.dat"), b"today's world").expect("a change");

    let stuck = super::with_suffix(&game.server_dir(), ".old");
    std::fs::write(&stuck, b"what the last restore could not clear away").expect("the leftover");

    let accepted = game
        .backups
        .restore(game.server, source, "safety net", game.owner)
        .await
        .expect("a restore");
    assert_eq!(game.await_operation(accepted.restore_operation_id).await, OperationState::Done);
    assert_eq!(
        std::fs::read(game.server_dir().join("world/level.dat")).expect("the world"),
        b"the old world",
        "10.6: the leftover of an earlier run is not a reason to refuse this one"
    );

    assert!(stuck.exists(), "the test's own premise — nothing could remove it");
    let console = game.operations.bus().channel(game.server).attach().history.lines;
    assert!(
        console.iter().any(|line| line.contains(".old") && line.contains("disk")),
        "the leftover goes into the console instead of a silent .ok(): {console:?}"
    );
}

#[tokio::test]
async fn a_hand_over_that_fails_leaves_the_old_server_where_it_stood() {
    let game = FakeServer::refusing_helper().await;
    game.file("world/level.dat", b"the old world");
    let source = game.a_finished_backup("one").await;
    std::fs::write(game.server_dir().join("world/level.dat"), b"today's world").expect("a change");

    let accepted = game
        .backups
        .restore(game.server, source, "safety net", game.owner)
        .await
        .expect("a restore");
    assert_eq!(game.await_operation(accepted.restore_operation_id).await, OperationState::Failed);

    assert_eq!(
        std::fs::read(game.server_dir().join("world/level.dat")).expect("the world"),
        b"today's world",
        "nothing was swapped, because the swap comes after the hand-over"
    );
    assert!(
        beside_the_server(&game).is_empty(),
        "the restore left something lying: {:?}",
        beside_the_server(&game)
    );
}

#[tokio::test]
async fn a_restore_whose_safety_copy_failed_leaves_the_server_alone() {
    let game = FakeServer::stopped().await;
    game.file("world/level.dat", b"the old world");
    let source = game.a_finished_backup("a good one").await;
    std::fs::write(game.server_dir().join("world/level.dat"), b"today's world").expect("a change");
    let parked = game.fill_the_panel().await;

    let accepted = game
        .backups
        .restore(game.server, source, "safety net", game.owner)
        .await
        .expect("a restore");

    let safety_run = sqlx::query_scalar::<_, Id>(
        "SELECT id FROM operations WHERE parent_operation_id = ?",
    )
    .bind(accepted.restore_operation_id)
    .fetch_one(game.pool())
    .await
    .expect("the safety run");
    game.operations
        .fail(
            safety_run,
            crate::model::OperationError {
                code: "no_space".to_owned(),
                message: "the disk is full".to_owned(),
                step: crate::model::OperationErrorStep::Filesystem,
            },
        )
        .await
        .expect("it fails");
    game.free_the_panel(parked).await;

    let ended = game.await_operation(accepted.restore_operation_id).await;

    assert_eq!(
        std::fs::read(game.server_dir().join("world/level.dat")).expect("the world"),
        b"today's world",
        "10.6: no safety copy, no restore"
    );
    assert_eq!(ended, OperationState::Failed);
    let run = game.operations.get(accepted.restore_operation_id).await.expect("the restore");
    assert_eq!(run.error.expect("an error").code, "safety_backup_failed");
}

#[tokio::test]
async fn an_archive_that_cannot_be_unpacked_leaves_the_old_directory_where_it_was() {
    let game = FakeServer::stopped().await;
    game.file("world/level.dat", b"today's world");
    let source = game.a_finished_backup("a good one").await;

    let path = game.backups.archive_of(game.server, source);
    evil_archive(&path);

    let accepted = game
        .backups
        .restore(game.server, source, "safety net", game.owner)
        .await
        .expect("a restore");
    assert_eq!(game.await_operation(accepted.restore_operation_id).await, OperationState::Failed);

    let run = game.operations.get(accepted.restore_operation_id).await.expect("the restore");
    assert_eq!(run.error.expect("an error").code, "invalid_path");
    assert_eq!(
        std::fs::read(game.server_dir().join("world/level.dat")).expect("the world"),
        b"today's world",
        "the swap happens after the whole archive is out, or not at all"
    );
    assert!(!game.data_dir().join("escaped.txt").exists());
    assert!(
        beside_the_server(&game).is_empty(),
        "the half unpacked directory is cleared away: {:?}",
        beside_the_server(&game)
    );
}

#[tokio::test]
async fn a_running_server_is_not_restored_onto() {
    let game = FakeServer::start().await;
    game.file("world/level.dat", b"a world");
    let source = game.a_finished_backup("one").await;

    let refused = game
        .backups
        .restore(game.server, source, "safety net", game.owner)
        .await
        .expect_err("10.6 wants a stopped server");
    assert_eq!(refused.code(), "server_running");
    assert_eq!(game.backup_rows().await, 1, "and no safety copy was made either");
}

#[tokio::test]
async fn an_unfinished_backup_is_neither_restorable_nor_downloadable() {
    let game = FakeServer::stopped().await;
    game.file("world/level.dat", b"a world");
    let queued = game
        .backups
        .create(game.server, "still going", Some(game.owner), false)
        .await
        .expect("a queued backup");

    assert_eq!(
        game.backups
            .restore(game.server, queued.backup, "safety net", game.owner)
            .await
            .expect_err("not finished")
            .code(),
        "backup_not_restorable",
        "10.6 weighs the backup before the lock on the server"
    );
    assert_eq!(
        game.backups.download(game.server, queued.backup).await.expect_err("not finished").code(),
        "backup_not_downloadable"
    );
}

#[tokio::test]
async fn a_cancelled_create_takes_its_row_and_its_part_file_with_it() {
    let game = FakeServer::stopped().await;
    game.file("world/level.dat", b"a world");
    let queued = game
        .backups
        .create(game.server, "on second thoughts", Some(game.owner), false)
        .await
        .expect("a queued backup");

    game.operations.cancelled(queued.operation).await.expect("it is called off");
    game.backups.run(queued.operation).await;

    assert_eq!(game.backup_rows().await, 0, "5.4: the row disappears with the part file");
    assert!(!game.backups.archive_of(game.server, queued.backup).exists());
}

#[tokio::test]
async fn the_quota_counts_the_running_backups_too() {
    let game = FakeServer::stopped().await;
    game.file("world/level.dat", b"a world");
    game.set_quota(2).await;

    game.a_finished_backup("one").await;
    let waiting = game
        .backups
        .create(game.server, "two", Some(game.owner), false)
        .await
        .expect("a second");

    let refused = game
        .backups
        .request(game.server, "three", game.owner)
        .await
        .expect_err("the quota is full");
    assert_eq!(
        refused.code(),
        "server_busy",
        "the second one is still open, and that is the nearer refusal"
    );

    game.backups.run(waiting.operation).await;
    let refused = game
        .backups
        .request(game.server, "three", game.owner)
        .await
        .expect_err("the quota is full");
    assert_eq!(refused.code(), "backup_limit_reached");
    assert_eq!(game.backup_rows().await, 2);
}

#[tokio::test]
async fn the_index_behind_the_check_refuses_in_the_words_of_10_2() {
    let game = FakeServer::stopped().await;
    game.file("world/level.dat", b"a world");
    game.backups.create(game.server, "one", Some(game.owner), false).await.expect("the first");

    let refused = game
        .backups
        .create(game.server, "two", Some(game.owner), false)
        .await
        .expect_err("one open create per server");
    assert_eq!(refused.code(), "server_busy");
    assert_eq!(refused.status(), axum::http::StatusCode::CONFLICT);
    assert_eq!(game.backup_rows().await, 1, "and the loser leaves no row behind");
}

#[tokio::test]
async fn two_requests_at_the_same_moment_leave_one_backup() {
    let game = FakeServer::stopped().await;
    game.file("world/level.dat", b"a world");
    game.fill_the_panel().await;

    let both = futures::future::join(
        game.backups.request(game.server, "left", game.owner),
        game.backups.request(game.server, "right", game.owner),
    )
    .await;

    let refusals: Vec<&str> = [&both.0, &both.1]
        .into_iter()
        .filter_map(|answer| answer.as_ref().err().map(|failure| failure.code()))
        .collect();
    assert_eq!(refusals, vec!["server_busy"], "10.2: the partial index is the second seam");
    assert_eq!(
        game.backup_rows().await,
        1,
        "and the loser leaves no half made row behind"
    );
}

#[tokio::test]
async fn a_second_backup_within_the_minute_is_asked_to_wait() {
    let game = FakeServer::stopped().await;
    game.file("world/level.dat", b"a world");
    let made = game.backups.request(game.server, "one", game.owner).await.expect("a backup");
    game.await_backup(made.id).await;

    let left = game.backups.cooldown(game.server).await.expect("an answer");
    assert!(matches!(left, Some(seconds) if seconds > 0 && seconds <= 61), "{left:?}");

    game.age_backup(made.id, 5).await;
    assert_eq!(game.backups.cooldown(game.server).await.expect("an answer"), None);
}

#[tokio::test]
async fn a_name_is_one_to_128_characters_and_a_long_one_still_fits_a_safety_copy() {
    assert_eq!(check_name("  tidy  ").expect("a name"), "tidy");
    assert_eq!(check_name("   ").expect_err("empty after trimming").code(), "invalid_name");
    assert_eq!(check_name("a\nb").expect_err("a control character").code(), "invalid_name");

    let long: String = "ä".repeat(MAX_NAME);
    assert!(check_name(&long).is_ok(), "128 characters, not 128 bytes");
    assert_eq!(check_name(&"a".repeat(MAX_NAME + 1)).expect_err("too long").code(), "invalid_name");

    let copy = safety_name_for(&long);
    assert_eq!(copy.chars().count(), SAFETY_NAME);
    assert!(check_name(&copy).is_ok());
}

#[tokio::test]
async fn the_list_is_newest_first_with_the_open_runs_beside_it() {
    let game = FakeServer::stopped().await;
    game.file("world/level.dat", b"a world");
    let older = game.a_finished_backup("older").await;
    game.age_backup(older, 30).await;
    let newer = game.a_finished_backup("newer").await;
    let running = game
        .backups
        .create(game.server, "under way", Some(game.owner), false)
        .await
        .expect("a queued backup");

    let list = game.backups.list(game.server).await.expect("a list");
    let order: Vec<Id> = list.backups.iter().map(|backup| backup.id).collect();
    assert_eq!(order, vec![running.backup, newer, older], "10.1: newest first");

    assert_eq!(list.active_operations.len(), 1);
    assert_eq!(list.active_operations[0].backup_id, running.backup);
    assert_eq!(list.active_operations[0].operation_id, running.operation);

    let waiting = &list.backups[0];
    assert_eq!(waiting.status, BackupStatus::Pending);
    assert_eq!(waiting.size_bytes, 0, "10.1: zero until it is done");
    assert_eq!(waiting.history.len(), 1);
    assert_eq!(waiting.history[0].state, BackupOperationState::Pending);
    assert!(waiting.history[0].should_prompt, "a run somebody started reports back");
    assert_eq!(
        waiting.history[0].user_info.as_ref().expect("a user").id,
        game.owner
    );

    let done = &list.backups[1];
    assert_eq!(done.status, BackupStatus::Done);
    assert!(done.size_bytes > 0);
}

#[tokio::test]
async fn a_history_stops_at_twenty_runs_and_keeps_the_newest_of_them() {
    let game = FakeServer::stopped().await;
    game.file("world/level.dat", b"a world");
    let backup = game.a_finished_backup("one").await;

    let mut made = Vec::new();
    for minute in 0..25 {
        let mut run = crate::ops::NewOperation::new(game.server, OperationKind::BackupCreate, None);
        run.target_id = Some(backup);
        let created = game.operations.create(run).await.expect("a run");
        game.operations.finish(created.id).await.expect("it ends");
        let when = Timestamp::at(
            Timestamp::now().as_datetime() + std::time::Duration::from_secs(60 * minute),
        );
        sqlx::query("UPDATE operations SET created_at = ? WHERE id = ?")
            .bind(when)
            .bind(created.id)
            .execute(game.pool())
            .await
            .expect("a run at its own minute");
        made.push(created.id);
    }

    let seen = game.backups.one(game.server, backup).await.expect("the backup");
    assert_eq!(seen.history.len(), super::store::HISTORY);
    let newest: Vec<Id> = seen.history.iter().map(|run| run.operation_id).collect();
    let expected: Vec<Id> = made.iter().rev().take(super::store::HISTORY).copied().collect();
    assert_eq!(newest, expected, "10.1: newest first, and the oldest fall out");
}

#[tokio::test]
async fn a_restore_makes_its_source_backup_look_busy() {
    let game = FakeServer::stopped().await;
    game.file("world/level.dat", b"a world");
    let source = game.a_finished_backup("one").await;

    let mut run = crate::ops::NewOperation::new(game.server, OperationKind::BackupRestore, None);
    run.target_id = Some(source);
    let restore = game.operations.create(run).await.expect("a restore run");
    game.operations.begin(restore.id).await.expect("no error").expect("it may start");

    let list = game.backups.list(game.server).await.expect("a list");
    let seen = list.backups.iter().find(|backup| backup.id == source).expect("the source");
    assert_eq!(
        seen.status,
        BackupStatus::InProgress,
        "10.1: hasRunningRestore crosses the run with the *source* backup's status"
    );
    assert_eq!(seen.size_bytes, 0, "10.1: zero whenever the status is not done");
}

#[tokio::test]
async fn repeating_a_failed_create_keeps_the_backup_id() {
    let game = FakeServer::stopped().await;
    game.file("world/level.dat", b"a world");
    let queued = game
        .backups
        .create(game.server, "first try", Some(game.owner), false)
        .await
        .expect("a queued backup");
    std::fs::create_dir_all(game.backups.archive_of(game.server, queued.backup))
        .expect("something in the way");
    game.backups.run(queued.operation).await;
    assert_eq!(
        crate::backups::store::one(game.pool(), queued.backup).await.expect("it").status,
        BackupStatus::Error
    );

    std::fs::remove_dir(game.backups.archive_of(game.server, queued.backup)).expect("out of the way");
    let again = game.backups.retry(game.server, queued.backup, game.owner).await.expect("a retry");
    assert_eq!(again.operation_type, BackupOperationType::Create);
    assert_ne!(again.operation_id, queued.operation, "10.7: a new run");
    assert_eq!(game.await_backup(queued.backup).await, BackupStatus::Done);
    assert_eq!(game.backup_rows().await, 1, "10.7: the same row, so the banner does not jump");
}

#[tokio::test]
async fn repeating_a_restore_uses_the_safety_copy_it_already_made() {
    let game = FakeServer::stopped().await;
    game.file("world/level.dat", b"a world");
    let source = game.a_finished_backup("one").await;

    let accepted = game
        .backups
        .restore(game.server, source, "safety net", game.owner)
        .await
        .expect("a restore");
    game.await_operation(accepted.restore_operation_id).await;

    game.operations
        .fail(
            accepted.restore_operation_id,
            crate::model::OperationError {
                code: "archive_corrupted".to_owned(),
                message: "half a file".to_owned(),
                step: crate::model::OperationErrorStep::Filesystem,
            },
        )
        .await
        .expect("it fails");

    let before = game.backup_rows().await;
    let again = game.backups.retry(game.server, source, game.owner).await.expect("a retry");
    assert_eq!(again.operation_type, BackupOperationType::Restore);
    assert_eq!(
        game.backup_rows().await,
        before,
        "10.7: a second copy on every click would eat the quota"
    );
    assert_eq!(game.await_operation(again.operation_id).await, OperationState::Done);
}

#[tokio::test]
async fn a_restore_retried_twice_still_makes_do_with_the_first_safety_copy() {
    let game = FakeServer::stopped().await;
    game.file("world/level.dat", b"a world");
    let source = game.a_finished_backup("one").await;

    let accepted = game
        .backups
        .restore(game.server, source, "safety net", game.owner)
        .await
        .expect("a restore");
    game.await_operation(accepted.restore_operation_id).await;

    let mut last = accepted.restore_operation_id;
    for attempt in 1..=2 {
        game.operations
            .fail(
                last,
                crate::model::OperationError {
                    code: "archive_corrupted".to_owned(),
                    message: "half a file".to_owned(),
                    step: crate::model::OperationErrorStep::Filesystem,
                },
            )
            .await
            .expect("it fails");

        let again = game.backups.retry(game.server, source, game.owner).await.expect("a retry");
        game.await_operation(again.operation_id).await;
        assert_eq!(
            game.backup_rows().await,
            2,
            "attempt {attempt}: the source and one safety copy, and nothing more"
        );
        last = again.operation_id;
    }
}

#[tokio::test]
async fn a_retry_that_cannot_afford_a_safety_copy_leaves_no_run_behind() {
    let game = FakeServer::stopped().await;
    game.file("world/level.dat", b"a world");
    let source = game.a_finished_backup("one").await;

    let accepted = game
        .backups
        .restore(game.server, source, "safety net", game.owner)
        .await
        .expect("a restore");
    game.await_operation(accepted.restore_operation_id).await;
    game.operations
        .fail(
            accepted.restore_operation_id,
            crate::model::OperationError {
                code: "archive_corrupted".to_owned(),
                message: "half a file".to_owned(),
                step: crate::model::OperationErrorStep::Filesystem,
            },
        )
        .await
        .expect("it fails");

    game.backups.delete(game.server, accepted.safety_backup.id).await.expect("a delete");
    game.set_quota(1).await;

    let refused = game
        .backups
        .retry(game.server, source, game.owner)
        .await
        .expect_err("the copy does not fit");
    assert_eq!(refused.code(), "backup_limit_reached");
    assert!(
        game.operations.guard_write(game.server).await.is_ok(),
        "a refusal must not leave a queued run holding the server: {:?}",
        game.operations.busy_reasons(game.server).await
    );
}

#[tokio::test]
async fn repeating_a_failed_create_asks_the_disk_pot_again() {
    const MIB: u64 = 1024 * 1024;
    let game = FakeServer::holding_disk(crate::auth::Disks::fixed(1024 * MIB, 0)).await;
    game.file("world/level.dat", b"a world");
    let queued = game
        .backups
        .create(game.server, "first try", Some(game.owner), false)
        .await
        .expect("a queued backup");
    let broken = game.backups.archive_of(game.server, queued.backup);
    std::fs::create_dir_all(&broken).expect("something in the way");
    game.backups.run(queued.operation).await;
    assert_eq!(
        crate::backups::store::one(game.pool(), queued.backup).await.expect("it").status,
        BackupStatus::Error
    );

    sqlx::query("UPDATE users SET disk_mib = 1024 WHERE id = ?")
        .bind(game.owner)
        .execute(game.pool())
        .await
        .unwrap();

    let refused =
        game.backups.retry(game.server, queued.backup, game.owner).await.expect_err("no room");
    assert_eq!(refused.code(), "disk_limit_reached");
    assert!(broken.exists(), "the door comes before the broken file is cleared away");
    assert!(
        game.operations.guard_write(game.server).await.is_ok(),
        "a refusal must not leave a queued run holding the server"
    );

    sqlx::query("UPDATE users SET disk_mib = 8192 WHERE id = ?")
        .bind(game.owner)
        .execute(game.pool())
        .await
        .unwrap();
    std::fs::remove_dir(&broken).expect("out of the way");
    game.backups.retry(game.server, queued.backup, game.owner).await.expect("a retry");
    assert_eq!(game.await_backup(queued.backup).await, BackupStatus::Done);
}

#[tokio::test]
async fn repeating_a_restore_that_needs_a_new_copy_asks_the_disk_pot_too() {
    const MIB: u64 = 1024 * 1024;
    let game = FakeServer::holding_disk(crate::auth::Disks::fixed(1024 * MIB, 0)).await;
    game.file("world/level.dat", b"a world");
    let source = game.a_finished_backup("one").await;

    let accepted = game
        .backups
        .restore(game.server, source, "safety net", game.owner)
        .await
        .expect("a restore");
    game.await_operation(accepted.restore_operation_id).await;
    game.operations
        .fail(
            accepted.restore_operation_id,
            crate::model::OperationError {
                code: "archive_corrupted".to_owned(),
                message: "half a file".to_owned(),
                step: crate::model::OperationErrorStep::Filesystem,
            },
        )
        .await
        .expect("it fails");
    game.backups.delete(game.server, accepted.safety_backup.id).await.expect("a delete");
    sqlx::query("UPDATE users SET disk_mib = 1024 WHERE id = ?")
        .bind(game.owner)
        .execute(game.pool())
        .await
        .unwrap();

    let refused = game.backups.retry(game.server, source, game.owner).await.expect_err("no room");
    assert_eq!(refused.code(), "disk_limit_reached");
    assert!(
        game.operations.guard_write(game.server).await.is_ok(),
        "a refusal must not leave a queued run holding the server: {:?}",
        game.operations.busy_reasons(game.server).await
    );
}

#[tokio::test]
async fn a_backup_whose_last_run_went_well_has_nothing_to_repeat() {
    let game = FakeServer::stopped().await;
    game.file("world/level.dat", b"a world");
    let backup = game.a_finished_backup("one").await;

    let refused = game
        .backups
        .retry(game.server, backup, game.owner)
        .await
        .expect_err("nothing failed");
    assert_eq!(refused.code(), "nothing_to_retry");
}

#[tokio::test]
async fn renaming_and_deleting_wait_for_the_run_on_that_backup() {
    let game = FakeServer::stopped().await;
    game.file("world/level.dat", b"a world");
    let queued = game
        .backups
        .create(game.server, "under way", Some(game.owner), false)
        .await
        .expect("a queued backup");

    assert_eq!(
        game.backups.rename(game.server, queued.backup, "new").await.expect_err("busy").code(),
        "server_busy"
    );
    assert_eq!(
        game.backups.delete(game.server, queued.backup).await.expect_err("busy").code(),
        "server_busy"
    );

    game.backups.run(queued.operation).await;
    let renamed =
        game.backups.rename(game.server, queued.backup, "  a better name ").await.expect("a rename");
    assert_eq!(renamed.name, "a better name");

    game.backups.delete(game.server, queued.backup).await.expect("a delete");
    assert_eq!(game.backup_rows().await, 0);
    assert!(
        !game.backups.archive_of(game.server, queued.backup).exists(),
        "10.4: no wastebasket, the dialogue says deletion is permanent"
    );
}

#[tokio::test]
async fn deleting_many_is_allowed_to_half_succeed() {
    let game = FakeServer::stopped().await;
    game.file("world/level.dat", b"a world");
    let good = game.a_finished_backup("one").await;
    let busy = game
        .backups
        .create(game.server, "under way", Some(game.owner), false)
        .await
        .expect("a queued backup");
    let stranger = Id::new();

    let (deleted, failed) =
        game.backups.delete_many(game.server, &[good, busy.backup, stranger]).await;
    assert_eq!(deleted, vec![good]);
    let codes: Vec<&str> = failed.iter().map(|failure| failure.error).collect();
    assert_eq!(codes, vec!["server_busy", "backup_not_found"]);
    assert_eq!(failed[1].id, stranger, "the caller is told which one stayed");
    assert_eq!(game.backup_rows().await, 1);
}

#[tokio::test]
async fn a_backup_of_another_server_is_no_backup_of_this_one() {
    let game = FakeServer::stopped().await;
    game.file("world/level.dat", b"a world");
    let backup = game.a_finished_backup("one").await;
    let other = crate::ops::testing::a_server(game.pool(), game.owner).await;

    assert_eq!(
        game.backups.one(other, backup).await.expect_err("not there").code(),
        "backup_not_found"
    );
    assert_eq!(
        game.backups.rename(other, backup, "mine now").await.expect_err("not there").code(),
        "backup_not_found"
    );

    let running = game
        .backups
        .create(game.server, "under way", Some(game.owner), false)
        .await
        .expect("a queued backup");
    assert_eq!(
        game.backups.download(other, running.backup).await.expect_err("not his").code(),
        "backup_not_found"
    );
}

#[test]
fn a_restore_that_runs_out_of_room_says_no_space_and_not_archive_corrupted() {
    let full = anyhow::Error::from(std::io::Error::from_raw_os_error(libc::ENOSPC))
        .context("unpacking world/region/r.0.0.mca");
    let Ended::Failed(error) = Ended::unpacking(&full) else {
        panic!("a full disk is a failure")
    };
    assert_eq!(error.code, "no_space");

    let Ended::Failed(error) = Ended::unpacking(&anyhow::anyhow!("unexpected end of file")) else {
        panic!("a broken archive is a failure")
    };
    assert_eq!(error.code, "archive_corrupted");
}

#[tokio::test]
async fn wiping_a_run_away_tells_the_page_to_look_again() {
    let game = FakeServer::stopped().await;
    game.file("world/level.dat", b"a world");
    let backup = game.a_finished_backup("one").await;
    let run = store::newest_run(game.pool(), backup)
        .await
        .expect("a query")
        .expect("a run")
        .operation_id;

    let mut listening = game.operations.channel(game.server).await.expect("a channel").attach();
    game.backups.dismiss(game.server, run).await;

    let mut told = false;
    while let Ok(event) = listening.events.try_recv() {
        if let crate::ops::ServerEvent::Say(json) = event {
            told |= json.contains("backup_list_changed");
        }
    }
    assert!(told, "the page has no other way of learning that the banner is gone");
    assert!(!game.backups.one(game.server, backup).await.expect("it").history[0].should_prompt);
}

#[tokio::test]
async fn a_restart_in_the_middle_of_a_backup_leaves_no_half_written_archive() {
    let game = FakeServer::stopped().await;
    game.file("world/level.dat", b"a world");
    let queued = game
        .backups
        .create(game.server, "interrupted", Some(game.owner), false)
        .await
        .expect("a queued backup");

    let part = game.backups.archive_of(game.server, queued.backup);
    std::fs::create_dir_all(part.parent().expect("a parent")).expect("the parents");
    std::fs::write(&part, b"half a frame").expect("a part file");

    game.backups.recover().await.expect("a sweep");
    assert!(
        !part.exists(),
        "5.12, backup_create: the started archive goes with the row, or it is never read again"
    );
}

#[tokio::test]
async fn a_restart_in_the_middle_of_a_backup_switches_saving_on_again() {
    let game = FakeServer::start().await;
    let mut run = crate::ops::NewOperation::new(game.server, OperationKind::BackupCreate, None);
    let row = game
        .backups
        .create(game.server, "interrupted", None, false)
        .await
        .expect("a queued backup");
    run.target_id = Some(row.backup);

    let swept = game.backups.recover().await.expect("a sweep");
    assert_eq!(swept, vec![game.server]);
    game.settle().await;
    assert_eq!(game.commands().await, vec!["save-on"]);
}

fn evil_archive(path: &std::path::Path) {
    let file = std::fs::File::create(path).expect("a file");
    let encoder = zstd::Encoder::new(file, 3).expect("an encoder");
    let mut builder = tar::Builder::new(encoder);
    let payload = b"owned";
    let mut header = tar::Header::new_gnu();
    let name = b"../../escaped.txt";
    header.as_old_mut().name[..name.len()].copy_from_slice(name);
    header.set_size(payload.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    builder.append(&header, &payload[..]).expect("an entry");
    builder.into_inner().expect("the encoder").finish().expect("zstd");
}

fn _assert_send_sync(backups: Arc<Backups>) -> Arc<dyn Send + Sync> {
    backups
}

#[tokio::test]
async fn a_backup_into_a_drive_leaves_nothing_on_our_disk() {
    let game = FakeServer::stopped().await;
    game.file("world/level.dat", b"a world worth keeping");
    game.connect_drive().await;
    game.aim_at_drive().await;

    let backup = game.a_finished_backup("to Google").await;
    assert_eq!(game.await_backup(backup).await, BackupStatus::Done);

    let row = store::find(game.pool(), backup).await.unwrap();
    assert_eq!(row.location, BackupLocation::Drive);
    assert_eq!(row.drive_state, Some(DriveFileState::Present));
    let file_id = row.drive_file_id.clone().expect("Google's file id");
    assert!(row.size_bytes > 0, "the size is kept — 10.1 shows it");

    let there = game.google().file_of_backup(backup).expect("the archive is in the Drive");
    assert_eq!(there.id, file_id);
    assert_eq!(there.bytes.len() as i64, row.size_bytes, "byte for byte the archive we packed");
    assert!(!there.name.contains(&backup.to_string()), "the name is for a person to read");
    assert!(!there.name.contains(&game.server.to_string()), "nor is the server a ULID here");
    assert!(there.name.starts_with("Survival--to-Google--"), "{}", there.name);
    assert!(there.name.ends_with(".tar.zst"));

    assert!(
        !game.backups.archive_of(game.server, backup).exists(),
        "the local archive stayed behind, and the disk it was meant to spare is filling up"
    );

    let listed = game.backups.list(game.server).await.unwrap();
    let seen = listed.backups.iter().find(|entry| entry.id == backup).expect("the row");
    assert_eq!(seen.location, BackupLocation::Drive);
    assert_eq!(
        seen.drive_web_link.as_deref(),
        Some(format!("https://drive.google.com/file/d/{file_id}/view").as_str())
    );
}

#[tokio::test]
async fn saving_is_switched_back_on_before_a_single_byte_goes_to_google() {
    let game = FakeServer::start().await;
    game.file("world/level.dat", b"a world worth keeping");
    game.connect_drive().await;
    game.aim_at_drive().await;
    game.google().hold_the_first_chunk(std::time::Duration::from_millis(300));

    let queued = game.backups.create(game.server, "in flight", Some(game.owner), false).await.unwrap();
    let backups = Arc::clone(&game.backups);
    let run = tokio::spawn(async move { backups.run(queued.operation).await });

    let mut seen = Vec::new();
    for _ in 0..600 {
        if game.google().chunks_seen() > 0 {
            seen = game.commands().await;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(game.google().chunks_seen() > 0, "no chunk ever reached Google");
    assert!(seen.contains(&"save-off".to_owned()), "it really was switched off for the packing");
    assert_eq!(
        seen.last().map(String::as_str),
        Some("save-on"),
        "the upload is running with saving switched off: {seen:?}"
    );

    run.await.unwrap();
    assert_eq!(game.await_backup(queued.backup).await, BackupStatus::Done);
}

#[tokio::test]
async fn a_multi_chunk_upload_survives_a_503_and_a_short_acknowledgement() {
    let game = FakeServer::stopped().await;
    game.file("world/region/r.0.0.mca", &noise(17 * 1024 * 1024));
    game.connect_drive().await;
    game.aim_at_drive().await;
    game.google().fail_chunk(2, 503);
    game.google().acknowledge_short_after(1);

    let backup = game.a_finished_backup("gross").await;
    assert_eq!(game.await_backup(backup).await, BackupStatus::Done, "a bad line is not a failure");

    let row = store::find(game.pool(), backup).await.unwrap();
    let there = game.google().file_of_backup(backup).expect("the archive is in the Drive");
    assert_eq!(there.bytes.len() as i64, row.size_bytes, "the file in the Drive is the wrong size");
    assert!(game.google().chunks_seen() >= 3, "17 MiB is more than one chunk");

    let unpacked = game.backups.dir_of(game.server).join("check.tar.zst");
    std::fs::write(&unpacked, &there.bytes).unwrap();
    let listed = std::process::Command::new("zstd")
        .args(["-t", unpacked.to_str().unwrap()])
        .output();
    if let Ok(output) = listed {
        assert!(output.status.success(), "the archive in the Drive does not decompress");
    }
}

#[tokio::test]
async fn a_full_drive_fails_the_run_at_once_and_says_so() {
    let game = FakeServer::stopped().await;
    game.file("world/level.dat", b"a world");
    game.connect_drive().await;
    game.aim_at_drive().await;
    game.google().fill_the_drive();

    let queued = game.backups.create(game.server, "too much", Some(game.owner), false).await.unwrap();
    game.backups.run(queued.operation).await;

    let run = game.operations.get(queued.operation).await.unwrap();
    assert_eq!(run.state, OperationState::Failed);
    let error = run.error.expect("an error");
    assert_eq!(error.code, "drive_quota_exceeded");
    assert_eq!(game.google().chunks_seen(), 1, "it was tried once and not five times");
    assert!(
        !game.backups.archive_of(game.server, queued.backup).exists(),
        "a failed run leaves no archive behind"
    );
    assert!(
        game.google().files().iter().all(|file| file.folder),
        "an archive appeared in the Drive for a run that failed: {:?}",
        game.google().files()
    );
}

#[tokio::test]
async fn a_backup_comes_back_out_of_the_drive_again() {
    let game = FakeServer::stopped().await;
    game.file("world/level.dat", b"the original world");
    game.connect_drive().await;
    game.aim_at_drive().await;

    let backup = game.a_finished_backup("for restoring").await;
    assert_eq!(game.await_backup(backup).await, BackupStatus::Done);

    std::fs::write(game.server_dir().join("world/level.dat"), b"ruined").unwrap();

    let accepted = game.backups.restore(game.server, backup, "safe", game.owner).await.unwrap();
    assert_eq!(
        game.await_operation(accepted.restore_operation_id).await,
        OperationState::Done,
        "the restore did not get through"
    );

    let level = std::fs::read(game.server_dir().join("world/level.dat")).unwrap();
    assert_eq!(level, b"the original world", "the world that came back is not the one we packed");
    assert!(
        !game.backups.archive_of(game.server, backup).exists(),
        "the copy that was brought down for the restore stayed on our disk"
    );
    assert!(
        game.google().file_of_backup(backup).is_some(),
        "and the archive is still in the user's Drive"
    );
}

#[tokio::test]
async fn a_backup_that_left_the_drive_cannot_be_restored() {
    let game = FakeServer::stopped().await;
    game.file("world/level.dat", b"a world");
    game.connect_drive().await;
    game.aim_at_drive().await;
    let backup = game.a_finished_backup("vanished").await;
    assert_eq!(game.await_backup(backup).await, BackupStatus::Done);

    let file = store::find(game.pool(), backup).await.unwrap().drive_file_id.unwrap();
    game.google().forget_file(&file);
    game.drive.of(game.owner).check().await.expect("a sweep");

    let refused = game.backups.restore(game.server, backup, "safe", game.owner).await.unwrap_err();
    assert_eq!(refused.code(), "backup_not_restorable");
    assert!(
        refused.to_string().contains("Google Drive"),
        "the sentence has to say where it went: {refused}"
    );

    game.backups.delete(game.server, backup).await.expect("the row may still go");
}

#[tokio::test]
async fn a_drive_backup_is_not_downloaded_through_the_panel() {
    let game = FakeServer::stopped().await;
    game.file("world/level.dat", b"a world");
    game.connect_drive().await;
    game.aim_at_drive().await;
    let backup = game.a_finished_backup("in the Drive").await;
    assert_eq!(game.await_backup(backup).await, BackupStatus::Done);

    let refused = game.backups.download(game.server, backup).await.unwrap_err();
    assert_eq!(refused.code(), "backup_lives_in_drive");
    assert_eq!(refused.status(), axum::http::StatusCode::CONFLICT);
    assert!(
        refused.to_string().contains("drive.google.com/file/d/"),
        "the refusal has to carry the way to the file: {refused}"
    );
}

#[tokio::test]
async fn a_backup_in_a_drive_does_not_hold_the_owners_disk_quota() {
    const MIB: u64 = 1024 * 1024;
    let game = FakeServer::stopped().await;
    sqlx::query("UPDATE users SET disk_mib = 1024 WHERE id = ?")
        .bind(game.owner)
        .execute(game.pool())
        .await
        .unwrap();

    let row = store::insert(game.pool(), game.server, "in the Drive", false, BackupLocation::Drive)
        .await
        .unwrap();
    store::finish_upload(game.pool(), row.id, "a-file-id", 1000 * MIB, Timestamp::now())
        .await
        .unwrap();

    let meter = || {
        crate::auth::Disks::over(
            game.pool().clone(),
            game.data_dir().to_path_buf(),
            std::time::Duration::from_millis(0),
            game.helper(),
        )
    };
    crate::auth::disk::guard(game.pool(), &meter(), game.owner, 100 * MIB)
        .await
        .expect("a backup in Google's hands must not fill the pot on this machine");

    let local =
        store::insert(game.pool(), game.server, "local and big", false, BackupLocation::Local)
            .await
            .unwrap();
    store::set_size(game.pool(), local.id, 1000 * MIB).await.unwrap();

    let refused = crate::auth::disk::guard(game.pool(), &meter(), game.owner, 100 * MIB)
        .await
        .expect_err("1000 MiB lying here should fill a 1024 MiB pot");
    assert_eq!(refused.code(), "disk_limit_reached");
}

#[tokio::test]
async fn the_safety_copy_of_a_restore_follows_the_servers_target() {
    let game = FakeServer::stopped().await;
    game.file("world/level.dat", b"a world");
    game.connect_drive().await;
    game.aim_at_drive().await;

    let backup = game.a_finished_backup("source").await;
    assert_eq!(game.await_backup(backup).await, BackupStatus::Done);

    let accepted = game.backups.restore(game.server, backup, "safety copy", game.owner).await.unwrap();
    assert_eq!(game.await_operation(accepted.restore_operation_id).await, OperationState::Done);

    let safety = store::find(game.pool(), accepted.safety_backup.id).await.unwrap();
    assert_eq!(safety.location, BackupLocation::Drive, "the safety copy landed on our own disk");
    assert!(safety.drive_file_id.is_some());
    assert!(!game.backups.archive_of(game.server, safety.id).exists());
}

#[tokio::test]
async fn switching_the_target_leaves_the_backups_that_exist_alone() {
    let game = FakeServer::stopped().await;
    game.file("world/level.dat", b"a world");
    let local = game.a_finished_backup("local first").await;
    assert_eq!(game.await_backup(local).await, BackupStatus::Done);
    assert!(game.backups.archive_of(game.server, local).exists());

    game.connect_drive().await;
    game.aim_at_drive().await;

    let row = store::find(game.pool(), local).await.unwrap();
    assert_eq!(row.location, BackupLocation::Local, "an existing row does not move");
    assert!(game.backups.archive_of(game.server, local).exists(), "and neither does its archive");
    assert!(game.google().files().is_empty(), "nothing was uploaded behind anybody's back");

    game.backups.download(game.server, local).await.expect("a local backup still downloads");
    let accepted = game.backups.restore(game.server, local, "safe", game.owner).await.unwrap();
    assert_eq!(game.await_operation(accepted.restore_operation_id).await, OperationState::Done);
}

#[tokio::test]
async fn drive_only_refuses_a_backup_rather_than_falling_back_to_our_disk() {
    let game = FakeServer::stopped().await;
    game.file("world/level.dat", b"a world");
    crate::drive::harness::with_credentials(&game.drive).await;
    game.drive
        .save(
            Some("1234.apps.googleusercontent.com".to_owned()),
            crate::drive::SecretChange::Keep,
            crate::model::BackupTargetPolicy::DriveOnly,
            "craftpanel-backups".to_owned(),
            Timestamp::now(),
        )
        .await
        .unwrap();

    let refused = game.backups.request(game.server, "nowhere", game.owner).await.unwrap_err();
    assert_eq!(refused.code(), "drive_not_connected");
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM backups")
        .fetch_one(game.pool())
        .await
        .unwrap();
    assert_eq!(rows, 0, "no row, and above all no archive on the disk the operator ruled out");
}

fn noise(bytes: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes);
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    while out.len() < bytes {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        out.extend_from_slice(&state.to_le_bytes());
    }
    out.truncate(bytes);
    out
}
