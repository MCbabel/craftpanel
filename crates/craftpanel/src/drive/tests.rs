use std::time::Duration;

use crate::model::{
    BackupLocation, BackupTargetPolicy, BackupTargetReason, DriveAccountState, DriveFileState,
    DriveLinkState, Id, PanelRole,
};
use crate::ops::testing::{a_server, a_user, cut_off, schema};

use super::harness::{self, DataDir, FakeGoogle};
use super::{Files, SecretChange};

async fn panel() -> (sqlx::SqlitePool, DataDir, FakeGoogle, std::sync::Arc<super::Drive>) {
    let pool = schema().await;
    let dir = DataDir::new();
    let google = FakeGoogle::started().await;
    let drive = harness::service(&pool, &dir, &google);
    (pool, dir, google, drive)
}

#[tokio::test]
async fn a_panel_with_nothing_set_up_answers_and_calls_nobody() {
    let (pool, _dir, google, drive) = panel().await;
    let anna = a_user(&pool, PanelRole::User).await;

    let status = drive.of(anna).status().await.expect("a status");
    assert!(!status.panel_configured, "the operator has entered nothing");
    assert!(!status.configured, "and so this account has connected nothing");
    assert_eq!(status.state, None);
    assert_eq!(status.folder_name, "craftpanel-backups", "the name is there to be shown");

    let refused = drive.of(anna).begin_link().await.expect_err("there is nothing to connect to");
    assert_eq!(refused.code(), "drive_not_configured");

    assert!(
        google.calls().is_empty(),
        "a panel with nothing set up called Google anyway: {:?}",
        google.calls()
    );
}

#[tokio::test]
async fn an_id_without_a_secret_is_not_a_setup() {
    let (pool, _dir, google, drive) = panel().await;
    let anna = a_user(&pool, PanelRole::User).await;

    drive
        .save(
            Some("1234.apps.googleusercontent.com".to_owned()),
            SecretChange::Keep,
            BackupTargetPolicy::UserChoice,
            "craftpanel-backups".to_owned(),
            crate::model::Timestamp::now(),
        )
        .await
        .expect("the id alone");

    assert!(!drive.panel_configured().await, "an id without a secret is half a setup");
    let refused = drive.of(anna).begin_link().await.expect_err("nothing to connect with");
    assert_eq!(refused.code(), "drive_not_configured");
    assert!(google.calls().is_empty());
}

#[tokio::test]
async fn saving_the_folder_name_does_not_take_the_secret_with_it() {
    let (_pool, dir, _google, drive) = panel().await;
    harness::with_credentials(&drive).await;
    let secret_file = dir.path().join("drive").join("client_secret");
    assert!(secret_file.exists(), "the secret is a file, not a row");

    let overview = drive
        .save(
            Some("1234.apps.googleusercontent.com".to_owned()),
            SecretChange::Keep,
            BackupTargetPolicy::DriveOnly,
            "somewhere-else".to_owned(),
            crate::model::Timestamp::now(),
        )
        .await
        .expect("a save that only changes the name");

    assert_eq!(overview.folder_name, "somewhere-else");
    assert_eq!(overview.target_policy, BackupTargetPolicy::DriveOnly);
    assert!(overview.configured, "the secret survived");
    assert!(secret_file.exists());

    let overview = drive
        .save(
            Some("1234.apps.googleusercontent.com".to_owned()),
            SecretChange::Remove,
            BackupTargetPolicy::UserChoice,
            "craftpanel-backups".to_owned(),
            crate::model::Timestamp::now(),
        )
        .await
        .expect("a save that removes it");
    assert!(!overview.configured);
    assert!(!secret_file.exists());
}

#[tokio::test]
async fn no_answer_of_the_admin_overview_can_carry_a_secret() {
    let (pool, _dir, _google, drive) = panel().await;
    harness::with_credentials(&drive).await;
    let anna = a_user(&pool, PanelRole::User).await;
    drive.of(anna).write_token("1//annas-refresh-token").await;

    let overview = drive.admin_overview().await.expect("the overview");
    let rendered = serde_json::to_string(&overview).expect("json");
    assert!(!rendered.contains("GOCSPX-test"), "the client secret is in the answer: {rendered}");
    assert!(!rendered.contains("annas-refresh-token"), "a token is in the answer: {rendered}");
    assert_eq!(overview.client_id.as_deref(), Some("1234.apps.googleusercontent.com"));
}

#[tokio::test]
async fn an_admin_never_sees_a_user_code() {
    let (pool, _dir, google, drive) = panel().await;
    harness::with_credentials(&drive).await;
    let anna = a_user(&pool, PanelRole::User).await;

    let started = drive.of(anna).begin_link().await.expect("an attempt");
    assert_eq!(started.user_code, "GQVQ-JKEC", "the owner does see his own code");
    assert_eq!(started.verification_url, "https://www.google.com/device");

    let overview = drive.admin_overview().await.expect("the overview");
    let rendered = serde_json::to_string(&overview).expect("json");
    assert!(
        !rendered.contains("GQVQ-JKEC"),
        "an admin answer carries a way into somebody's Google account: {rendered}"
    );
    drive.of(anna).cancel_link().await.expect("cancelling");
    let _ = google;
}

#[tokio::test]
async fn a_code_that_somebody_confirms_leaves_a_token_on_disk() {
    let (pool, dir, google, drive) = panel().await;
    harness::with_credentials(&drive).await;
    let anna = a_user(&pool, PanelRole::User).await;
    google.keep_waiting(2);

    let started = drive.of(anna).begin_link().await.expect("an attempt");
    assert_eq!(started.state, DriveLinkState::Waiting);
    assert!(started.expires_at > started.started_at, "Google's own deadline, not ours");

    let token = dir.path().join("drive").join(anna.to_string()).join("refresh_token");
    for _ in 0..400 {
        if token.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(token.exists(), "the loop never picked the token up");

    for _ in 0..400 {
        if google.calls().contains(&"about".to_owned()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let status = drive.of(anna).status().await.expect("a status");
    assert!(status.configured, "a token is here");
    assert_eq!(status.state, Some(DriveAccountState::Connected));
    assert_eq!(status.google_email.as_deref(), Some("anna@example.com"), "about.get ran once");
    assert_eq!(status.storage_limit_bytes, Some(16_106_127_360));

    let over = drive.of(anna).link().await.expect_err("the attempt is over");
    assert_eq!(over.code(), "drive_link_not_found");

    let again = drive.of(anna).begin_link().await.expect_err("already connected");
    assert_eq!(again.code(), "drive_already_linked");
}

#[tokio::test]
async fn a_declined_request_says_so_and_leaves_no_token() {
    let (pool, dir, google, drive) = panel().await;
    harness::with_credentials(&drive).await;
    let anna = a_user(&pool, PanelRole::User).await;
    google.decline_the_code();

    drive.of(anna).begin_link().await.expect("an attempt");
    let seen = settled(&drive, anna).await;
    assert_eq!(seen, Some(DriveLinkState::Denied), "the row has to name the way it ended");
    assert!(!dir.path().join("drive").join(anna.to_string()).join("refresh_token").exists());

    let status = drive.of(anna).status().await.expect("a status");
    assert!(!status.configured);
    let complaint = status.last_error.expect("a failed attempt names its reason");
    assert!(complaint.contains("access_denied"), "{complaint}");
    assert!(complaint.contains("Test users"), "{complaint}");
    assert_eq!(status.state, None, "and a refused attempt is still not a broken connection");
}

#[tokio::test]
async fn an_account_that_is_only_connecting_is_no_error_anywhere() {
    let (pool, _dir, google, drive) = panel().await;
    harness::with_credentials(&drive).await;
    google.keep_waiting(1_000);
    let anna = a_user(&pool, PanelRole::User).await;

    drive.of(anna).begin_link().await.expect("an attempt");

    let status = drive.of(anna).status().await.expect("a status");
    assert_eq!(status.state, None, "beginning an attempt is not a fault");
    assert_eq!(status.last_error, None, "and there is nothing to complain about yet");
    assert!(!status.configured);
    assert_eq!(
        status.link.map(|link| link.state),
        Some(DriveLinkState::Waiting),
        "what it *is* doing belongs in `link`, which is where playit keeps its claim too"
    );

    let line = drive
        .admin_overview()
        .await
        .expect("the overview")
        .accounts
        .pop()
        .expect("a row while the attempt runs");
    assert_eq!(line.state, None, "the operator saw `error` here for a user who was connecting");
    assert_eq!(line.last_error, None);
}

#[tokio::test]
async fn a_refusal_the_operator_has_to_fix_says_what_to_change() {
    let (pool, _dir, google, drive) = panel().await;
    harness::with_credentials(&drive).await;
    let anna = a_user(&pool, PanelRole::User).await;
    google.end_the_flow_with(400, include_str!("testdata/admin_policy_enforced.json"));

    drive.of(anna).begin_link().await.expect("an attempt");
    assert_eq!(settled(&drive, anna).await, Some(DriveLinkState::Denied));

    let complaint =
        drive.of(anna).status().await.expect("a status").last_error.expect("a sentence");
    assert!(complaint.contains("admin_policy_enforced"), "Google's word: {complaint}");
    assert!(complaint.contains("Workspace administrator"), "and ours: {complaint}");
}

#[tokio::test]
async fn a_day_spent_at_google_turns_the_backup_button_away_before_it_packs_anything() {
    let (pool, _dir, _google, drive) = panel().await;
    harness::with_credentials(&drive).await;
    let anna = a_user(&pool, PanelRole::User).await;
    let server = a_server(&pool, anna).await;
    drive.of(anna).write_token("1//a-token").await;
    super::store::set_target(
        &pool,
        server,
        BackupLocation::Drive,
        crate::model::Timestamp::now(),
    )
    .await
    .expect("a server that backs up into the Drive");

    drive.guard_backup(server).await.expect("the day is still young");

    let now = crate::model::Timestamp::now();
    super::store::note_sent(&pool, anna, &super::day::day_of(now), super::day::CEILING, now)
        .await
        .expect("a day that is spent");

    let refused = drive.guard_backup(server).await.expect_err("Google takes nothing more today");
    assert_eq!(refused.code(), "drive_day_full");
    assert_eq!(refused.status(), axum::http::StatusCode::TOO_MANY_REQUESTS);
    assert!(
        refused.to_string().contains("750 GB"),
        "the button says nothing about why: {refused}"
    );
}

#[tokio::test]
async fn a_withdrawn_connection_is_no_target_and_the_page_is_told_so() {
    let (pool, _dir, google, drive) = panel().await;
    harness::with_credentials(&drive).await;
    let anna = a_user(&pool, PanelRole::User).await;
    let server = a_server(&pool, anna).await;
    drive.of(anna).write_token("1//a-token").await;
    drive
        .save(
            Some("1234.apps.googleusercontent.com".to_owned()),
            SecretChange::Keep,
            BackupTargetPolicy::DriveOnly,
            "craftpanel-backups".to_owned(),
            crate::model::Timestamp::now(),
        )
        .await
        .expect("the operator's rule");
    drive.guard_backup(server).await.expect("while the connection carries");

    google.withdraw_the_connection();
    let status = drive.of(anna).check().await.expect("a status rather than an error");
    assert_eq!(status.state, Some(DriveAccountState::Revoked));
    assert!(status.configured, "the key file stays; the state is in the column");

    let target = drive.target_of(server).await.expect("a target");
    assert_eq!(target.reason, BackupTargetReason::NotConnected, "the page can say why");
    let refused = drive.guard_backup(server).await.expect_err("nothing to upload into");
    assert_eq!(refused.code(), "drive_not_connected");
}

#[tokio::test]
async fn the_panel_wide_switch_closes_everything_but_the_way_out() {
    let (pool, _dir, google, drive) = panel().await;
    harness::with_credentials(&drive).await;
    let anna = a_user(&pool, PanelRole::User).await;
    drive.of(anna).write_token("1//a-token").await;
    switch_off(&pool).await;

    let refused = drive.of(anna).begin_link().await.expect_err("the switch is off");
    assert_eq!(refused.code(), "external_services_disabled");
    let refused = drive.of(anna).check().await.expect_err("the switch is off");
    assert_eq!(refused.code(), "external_services_disabled");

    assert!(drive.of(anna).status().await.expect("a status").configured);

    drive.of(anna).disconnect(Files::Keep).await.expect("the way out of the corner");
    assert!(!drive.of(anna).status().await.expect("a status").configured);
    assert!(
        google.calls().is_empty(),
        "keep must not call out with the switch off: {:?}",
        google.calls()
    );
}

#[tokio::test]
async fn a_server_is_local_until_the_panel_and_the_account_are_both_ready() {
    let (pool, _dir, _google, drive) = panel().await;
    let anna = a_user(&pool, PanelRole::User).await;
    let server = a_server(&pool, anna).await;

    let target = drive.target_of(server).await.expect("a target");
    assert_eq!(target.effective_target, BackupLocation::Local);
    assert_eq!(target.reason, BackupTargetReason::NotConfigured, "and it says why");

    let refused = drive.set_target(server, BackupLocation::Drive).await.expect_err("no project");
    assert_eq!(refused.code(), "drive_not_configured");

    harness::with_credentials(&drive).await;
    let target = drive.target_of(server).await.expect("a target");
    assert_eq!(target.reason, BackupTargetReason::NotConnected, "now it is the account's turn");
    let refused = drive.set_target(server, BackupLocation::Drive).await.expect_err("no account");
    assert_eq!(refused.code(), "drive_not_connected");

    drive.of(anna).write_token("1//a-token").await;
    let switched = drive.set_target(server, BackupLocation::Drive).await.expect("both ready");
    assert_eq!(switched.target, BackupLocation::Drive);
    assert_eq!(switched.effective_target, BackupLocation::Drive);
    assert_eq!(switched.reason, BackupTargetReason::Ok);
}

#[tokio::test]
async fn the_operators_rule_decides_what_a_server_may_choose() {
    let (pool, _dir, _google, drive) = panel().await;
    harness::with_credentials(&drive).await;
    let anna = a_user(&pool, PanelRole::User).await;
    let server = a_server(&pool, anna).await;

    let policy = |policy| {
        let drive = std::sync::Arc::clone(&drive);
        async move {
            drive
                .save(
                    Some("1234.apps.googleusercontent.com".to_owned()),
                    SecretChange::Keep,
                    policy,
                    "craftpanel-backups".to_owned(),
                    crate::model::Timestamp::now(),
                )
                .await
                .expect("the rule");
        }
    };

    policy(BackupTargetPolicy::LocalOnly).await;
    let refused = drive.set_target(server, BackupLocation::Drive).await.expect_err("local only");
    assert_eq!(refused.code(), "target_not_allowed");
    assert_eq!(
        drive.target_of(server).await.expect("a target").reason,
        BackupTargetReason::Policy
    );

    policy(BackupTargetPolicy::DriveOnly).await;
    let refused = drive.set_target(server, BackupLocation::Local).await.expect_err("drive only");
    assert_eq!(refused.code(), "target_not_allowed");
    let target = drive.target_of(server).await.expect("a target");
    assert_eq!(target.reason, BackupTargetReason::NotConnected);
    let refused = drive.guard_backup(server).await.expect_err("drive only, nothing connected");
    assert_eq!(refused.code(), "drive_not_connected");

    drive.of(anna).write_token("1//a-token").await;
    let target = drive.target_of(server).await.expect("a target");
    assert_eq!(target.effective_target, BackupLocation::Drive, "drive_only means drive");
    drive.guard_backup(server).await.expect("now it may run");
}

#[tokio::test]
async fn the_target_asks_about_the_owner_and_not_the_caller() {
    let (pool, _dir, _google, drive) = panel().await;
    harness::with_credentials(&drive).await;
    let anna = a_user(&pool, PanelRole::User).await;
    let ben = a_user(&pool, PanelRole::User).await;
    let hers = a_server(&pool, anna).await;

    drive.of(ben).write_token("1//bens-token").await;
    let refused = drive.set_target(hers, BackupLocation::Drive).await.expect_err("not hers");
    assert_eq!(refused.code(), "drive_not_connected");
}

#[tokio::test]
async fn the_sweep_notices_a_file_that_was_deleted_or_binned() {
    let (pool, _dir, google, drive) = panel().await;
    harness::with_credentials(&drive).await;
    let anna = a_user(&pool, PanelRole::User).await;
    let server = a_server(&pool, anna).await;
    drive.of(anna).write_token("1//a-token").await;

    let present = a_drive_backup(&pool, server, &google, "present").await;
    let binned = a_drive_backup(&pool, server, &google, "binned").await;
    let gone = a_drive_backup(&pool, server, &google, "gone").await;
    google.trash_file(&file_of(&pool, binned).await);
    google.forget_file(&file_of(&pool, gone).await);

    drive.of(anna).check().await.expect("a check");

    assert_eq!(state_of(&pool, present).await, Some(DriveFileState::Present));
    assert_eq!(state_of(&pool, binned).await, Some(DriveFileState::Trashed));
    assert_eq!(state_of(&pool, gone).await, Some(DriveFileState::Missing));
}

#[tokio::test]
async fn the_sweep_takes_an_orphan_and_never_a_strangers_file() {
    let (pool, _dir, google, drive) = panel().await;
    harness::with_credentials(&drive).await;
    let anna = a_user(&pool, PanelRole::User).await;
    let server = a_server(&pool, anna).await;
    drive.of(anna).write_token("1//a-token").await;

    let mine = a_drive_backup(&pool, server, &google, "mine").await;
    let orphan = google.put_orphan(&Id::new().to_string());
    let stranger = google.put_stranger("holiday.jpg");

    drive.of(anna).check().await.expect("a check");

    let left: Vec<String> = google.files().into_iter().map(|file| file.id).collect();
    assert!(left.contains(&stranger), "a file that is not ours was deleted: {left:?}");
    assert!(!left.contains(&orphan), "an archive nobody points at stayed behind: {left:?}");
    assert!(left.contains(&file_of(&pool, mine).await), "and the one with a row stays");
}

#[tokio::test]
async fn a_withdrawn_connection_is_written_down_and_the_key_file_stays() {
    let (pool, dir, google, drive) = panel().await;
    harness::with_credentials(&drive).await;
    let anna = a_user(&pool, PanelRole::User).await;
    drive.of(anna).write_token("1//a-token").await;
    google.withdraw_the_connection();

    let status = drive.of(anna).check().await.expect("a status rather than an error");
    assert_eq!(status.state, Some(DriveAccountState::Revoked));
    let complaint = status.last_error.expect("a sentence");
    assert!(
        complaint.contains("Testing"),
        "a connection this young is the seven-day trap, and the sentence has to say so: \
         {complaint}"
    );

    let token = dir.path().join("drive").join(anna.to_string()).join("refresh_token");
    assert!(token.exists(), "the key file was thrown away instead of the state being written");
    assert!(status.configured, "and `configured` still says a token is here");
}

#[tokio::test]
async fn letting_go_with_backups_in_the_drive_asks_first() {
    let (pool, _dir, google, drive) = panel().await;
    harness::with_credentials(&drive).await;
    let anna = a_user(&pool, PanelRole::User).await;
    let server = a_server(&pool, anna).await;
    drive.of(anna).write_token("1//a-token").await;
    let backup = a_drive_backup(&pool, server, &google, "one").await;

    let refused = drive.of(anna).disconnect(Files::Refuse).await.expect_err("it has to ask");
    assert_eq!(refused.code(), "drive_has_backups");

    drive.of(anna).disconnect(Files::Keep).await.expect("keep");
    assert_eq!(
        state_of(&pool, backup).await,
        Some(DriveFileState::Unreachable),
        "the row stays and says the panel cannot see the file any more"
    );
    assert_eq!(google.files().len(), 1, "and the archive is still in the user's Drive");
    assert!(google.calls().contains(&"revoke".to_owned()), "the token is handed back to Google");
}

#[tokio::test]
async fn letting_go_with_delete_takes_the_archives_and_the_rows() {
    let (pool, _dir, google, drive) = panel().await;
    harness::with_credentials(&drive).await;
    let anna = a_user(&pool, PanelRole::User).await;
    let server = a_server(&pool, anna).await;
    drive.of(anna).write_token("1//a-token").await;
    a_drive_backup(&pool, server, &google, "one").await;
    a_drive_backup(&pool, server, &google, "two").await;

    drive.of(anna).disconnect(Files::Delete).await.expect("delete");

    assert!(google.files().is_empty(), "the archives are gone from the Drive");
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM backups WHERE server_id = ?")
        .bind(server)
        .fetch_one(&pool)
        .await
        .expect("a count");
    assert_eq!(rows, 0, "and so are the rows that named them");
    assert!(!drive.of(anna).status().await.expect("a status").configured);
}

#[tokio::test]
async fn an_admin_disconnecting_somebody_leaves_every_file_alone() {
    let (pool, _dir, google, drive) = panel().await;
    harness::with_credentials(&drive).await;
    let anna = a_user(&pool, PanelRole::User).await;
    let server = a_server(&pool, anna).await;
    drive.of(anna).write_token("1//a-token").await;
    let backup = a_drive_backup(&pool, server, &google, "hers").await;

    drive.of(anna).disconnect(Files::Keep).await.expect("cutting her loose");

    assert_eq!(google.files().len(), 1, "her archive is untouched");
    assert_eq!(state_of(&pool, backup).await, Some(DriveFileState::Unreachable));
    assert!(!google.calls().contains(&"files/delete".to_owned()), "nothing of hers was deleted");
}

#[tokio::test]
async fn deleting_an_account_takes_its_google_token_off_the_disk_and_gives_it_back() {
    use crate::auth::harness::{an_admin, as_user, empty, sign_in, state_with, FakeHelper};
    use tower::ServiceExt;

    let (pool, dir, google, drive) = panel().await;
    harness::with_credentials(&drive).await;
    let anna = a_user(&pool, PanelRole::User).await;
    drive.of(anna).write_token("1//annas-token").await;

    let token = dir.path().join("drive").join(anna.to_string()).join("refresh_token");
    assert!(token.exists(), "her token is on the disk");

    let helper = FakeHelper::obliging().await;
    let config = crate::config::Config {
        helper_socket: helper.socket(),
        ..crate::config::Config::default()
    };
    let playit = crate::playit::Playit::against(
        pool.clone(),
        std::sync::Arc::new(config.clone()),
        "http://127.0.0.1:1",
    )
    .expect("the playit service");
    let app = crate::api::admin::with_live(
        crate::auth::LiveServers::none(),
        crate::auth::Disks::none(),
        playit,
        std::sync::Arc::clone(&drive),
    )
    .with_state(state_with(&pool, config));

    let boss = an_admin(&pool, "boss").await;
    let cookie = sign_in(&pool, boss).await;
    let gone = app
        .oneshot(as_user(empty("DELETE", &format!("/admin/users/{anna}")), &cookie))
        .await
        .expect("an answer");
    assert_eq!(gone.status(), axum::http::StatusCode::NO_CONTENT);

    assert!(!token.exists(), "her refresh token is still on the disk");
    assert!(
        !dir.path().join("drive").join(anna.to_string()).exists(),
        "her directory is still there"
    );
    assert!(google.calls().contains(&"revoke".to_owned()), "{:?}", google.calls());
}

#[tokio::test]
async fn a_bad_moment_at_google_is_ridden_out_instead_of_ending_the_run() {
    let (pool, _dir, google, drive) = panel().await;
    harness::with_credentials(&drive).await;
    let anna = a_user(&pool, PanelRole::User).await;
    let server = a_server(&pool, anna).await;
    drive.of(anna).write_token("1//a-token").await;
    let backup = a_drive_backup(&pool, server, &google, "one").await;
    let file = file_of(&pool, backup).await;

    google.turn_away("token/refresh", 1, 503, None);
    google.turn_away("files/get", 2, 429, None);

    let seen = drive.size_of(server, &file).await.expect("a 429 is a moment, not an answer");
    assert_eq!(seen.id, file);

    let calls = google.calls();
    assert_eq!(
        calls.iter().filter(|call| *call == "token/refresh").count(),
        2,
        "a 503 on the token was taken for a final word: {calls:?}"
    );
    assert_eq!(
        calls.iter().filter(|call| *call == "files/get").count(),
        3,
        "the two rate limits were not ridden out: {calls:?}"
    );
}

#[tokio::test]
async fn a_full_drive_is_never_asked_a_second_time() {
    let (pool, _dir, google, drive) = panel().await;
    harness::with_credentials(&drive).await;
    let anna = a_user(&pool, PanelRole::User).await;
    let server = a_server(&pool, anna).await;
    drive.of(anna).write_token("1//a-token").await;
    let backup = a_drive_backup(&pool, server, &google, "one").await;
    let file = file_of(&pool, backup).await;

    google.turn_away_with_body(
        "files/get",
        5,
        403,
        include_str!("testdata/storage_quota_exceeded.json"),
    );

    let err = drive.size_of(server, &file).await.expect_err("a full Drive does not empty itself");
    assert_eq!(err.operation_code(), "drive_quota_exceeded");
    assert_eq!(
        google.calls().iter().filter(|call| *call == "files/get").count(),
        1,
        "a full Drive was asked again and again: {:?}",
        google.calls()
    );
}

#[tokio::test]
async fn the_wait_google_asks_for_is_the_wait_it_gets() {
    let (pool, _dir, google, drive) = panel().await;
    harness::with_credentials(&drive).await;
    let anna = a_user(&pool, PanelRole::User).await;
    let server = a_server(&pool, anna).await;
    drive.of(anna).write_token("1//a-token").await;
    let backup = a_drive_backup(&pool, server, &google, "one").await;
    let file = file_of(&pool, backup).await;

    google.turn_away("files/get", 1, 429, Some("1"));

    let started = std::time::Instant::now();
    drive.size_of(server, &file).await.expect("the second try got through");
    assert!(
        started.elapsed() >= Duration::from_millis(900),
        "Google asked for a second and got {:?}",
        started.elapsed()
    );
}

async fn a_drive_backup(
    pool: &sqlx::SqlitePool,
    server: Id,
    google: &FakeGoogle,
    name: &str,
) -> Id {
    let backup = crate::backups::store::insert(pool, server, name, false, BackupLocation::Drive)
        .await
        .expect("a row");
    let file = google.put_orphan(&backup.id.to_string());
    crate::backups::store::finish_upload(
        pool,
        backup.id,
        &file,
        27,
        None,
        crate::model::Timestamp::now(),
    )
    .await
    .expect("a finished upload");
    backup.id
}

async fn settled(drive: &std::sync::Arc<super::Drive>, user: Id) -> Option<DriveLinkState> {
    for _ in 0..400 {
        tokio::time::sleep(Duration::from_millis(25)).await;
        if let Ok(link) = drive.of(user).link().await {
            if link.state != DriveLinkState::Waiting {
                return Some(link.state);
            }
        }
    }
    None
}

async fn file_of(pool: &sqlx::SqlitePool, backup: Id) -> String {
    sqlx::query_scalar("SELECT drive_file_id FROM backups WHERE id = ?")
        .bind(backup)
        .fetch_one(pool)
        .await
        .expect("a file id")
}

async fn state_of(pool: &sqlx::SqlitePool, backup: Id) -> Option<DriveFileState> {
    sqlx::query_scalar("SELECT drive_state FROM backups WHERE id = ?")
        .bind(backup)
        .fetch_one(pool)
        .await
        .expect("a state")
}

async fn switch_off(pool: &sqlx::SqlitePool) {
    sqlx::query("UPDATE panel_settings SET external_services_enabled = 0 WHERE id = 1")
        .execute(pool)
        .await
        .expect("the switch");
}

async fn a_drive_row(pool: &sqlx::SqlitePool, server: Id, name: &str) -> Id {
    crate::backups::store::insert(pool, server, name, false, BackupLocation::Drive)
        .await
        .expect("a backup row")
        .id
}

fn sha256_of(bytes: &[u8]) -> String {
    use sha2::Digest;
    let mut digest = sha2::Sha256::new();
    digest.update(bytes);
    hex::encode(digest.finalize())
}

fn md5_of(bytes: &[u8]) -> String {
    use md5::Digest;
    let mut digest = md5::Md5::new();
    digest.update(bytes);
    hex::encode(digest.finalize())
}

fn filler(bytes: usize, seed: u64) -> Vec<u8> {
    (0..bytes)
        .map(|at| ((at as u64).wrapping_mul(2_654_435_761).wrapping_add(seed) >> 11) as u8)
        .collect()
}

fn address_of(dir: &DataDir, user: Id, backup: Id) -> std::path::PathBuf {
    dir.path()
        .join("drive")
        .join(user.to_string())
        .join("sessions")
        .join(backup.to_string())
}

fn mode_of(path: &std::path::Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).expect("the path exists").permissions().mode() & 0o777
}

fn touch_later(path: &std::path::Path) {
    let file = std::fs::File::options().write(true).open(path).expect("the archive");
    let later = std::time::SystemTime::now() + Duration::from_secs(5);
    file.set_times(std::fs::FileTimes::new().set_modified(later))
        .expect("a later modification time");
}

fn start_sending(
    drive: &std::sync::Arc<super::Drive>,
    server: Id,
    backup: Id,
    archive: &std::path::Path,
    size: u64,
) -> tokio::task::JoinHandle<()> {
    let drive = std::sync::Arc::clone(drive);
    let archive = archive.to_path_buf();
    tokio::spawn(async move {
        let progress = std::sync::Arc::new(crate::backups::archive::Progress::default());
        let _ = drive
            .upload_archive(server, backup, &archive, size, "monday.tar.zst", &progress)
            .await;
    })
}

async fn wait_for_chunks(google: &FakeGoogle, how_many: usize) {
    for _ in 0..2000 {
        if google.chunks_seen() >= how_many {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("Google never saw {how_many} chunks of the archive");
}

async fn session_of(pool: &sqlx::SqlitePool, backup: Id) -> Option<super::store::Upload> {
    super::store::upload_of(pool, backup).await.expect("no error")
}

struct Interrupted {
    pool: sqlx::SqlitePool,
    dir: DataDir,
    google: FakeGoogle,
    anna: Id,
    server: Id,
    backup: Id,
    archive: std::path::PathBuf,
    whole: Vec<u8>,
}

async fn an_upload_stopped_by_a_restart(chunks: usize) -> Interrupted {
    stopped_after(chunks, None).await
}

async fn an_upload_stopped_with_the_last_chunk_in_the_air() -> Interrupted {
    stopped_after(3, Some(3)).await
}

async fn stopped_after(chunks: usize, held: Option<usize>) -> Interrupted {
    let (pool, dir, google, drive) = panel().await;
    harness::with_credentials(&drive).await;
    let anna = a_user(&pool, PanelRole::User).await;
    let server = a_server(&pool, anna).await;
    drive.of(anna).write_token("1//a-token").await;

    let whole = filler((super::upload::CHUNK * 2 + 4096) as usize, 1);
    tokio::fs::create_dir_all(dir.path()).await.expect("a place for the archive");
    let archive = dir.path().join("monday.tar.zst");
    tokio::fs::write(&archive, &whole).await.expect("an archive on the disk");
    let backup = a_drive_row(&pool, server, "Monday").await;

    if let Some(number) = held {
        google.hold_the_chunk(number, Duration::from_secs(10));
    }
    let sending = start_sending(&drive, server, backup, &archive, whole.len() as u64);
    wait_for_chunks(&google, chunks).await;
    cut_off(sending, &pool).await;

    Interrupted { pool, dir, google, anna, server, backup, archive, whole }
}

#[tokio::test]
async fn an_upload_carries_on_where_the_restart_left_it() {
    let stopped = an_upload_stopped_by_a_restart(2).await;
    let Interrupted { pool, dir, google, anna, server, backup, archive, whole } = stopped;

    let row = session_of(&pool, backup).await.expect("the session outlived the run");
    assert_eq!(row.user_id, anna, "it says whose Drive it writes into");
    assert_eq!(row.total_bytes as u64, whole.len() as u64);
    assert!(address_of(&dir, anna, backup).exists(), "the address is on the disk, not in the air");
    let stopped_after = google.chunks_seen();

    let after_restart = harness::service(&pool, &dir, &google);
    let progress = std::sync::Arc::new(crate::backups::archive::Progress::default());
    let stored = after_restart
        .upload_archive(server, backup, &archive, whole.len() as u64, "monday.tar.zst", &progress)
        .await
        .expect("the rest of the upload");

    let file = google.file_of_backup(backup).expect("the archive in her Drive");
    assert_eq!(file.id, stored.file_id);
    assert_eq!(file.bytes, whole, "the file in Drive is not the archive that was on the disk");
    assert_eq!(
        stored.md5.as_deref(),
        Some(md5_of(&whole).as_str()),
        "the checksum has to cover the bytes that went up before the restart as well, or a \
         resumed upload can never be confirmed"
    );
    assert!(
        google.chunks_seen() < stopped_after + 3,
        "the whole archive went up a second time instead of only the rest: {} chunks",
        google.chunks_seen()
    );
    assert_eq!(
        progress.bytes(),
        whole.len() as u64,
        "the bar has to count what Google already holds, or it lies about the rest"
    );
    assert!(session_of(&pool, backup).await.is_none(), "a spent session is not kept");
    assert!(!address_of(&dir, anna, backup).exists(), "and neither is its address");
}

#[tokio::test]
async fn half_of_one_archive_is_never_glued_to_half_of_another() {
    let stopped = an_upload_stopped_by_a_restart(2).await;
    let Interrupted { pool, dir, google, anna, server, backup, archive, whole } = stopped;
    assert!(session_of(&pool, backup).await.is_some(), "there is something to be tempted by");

    let repacked = filler(whole.len(), 7);
    assert_ne!(repacked, whole, "the two worlds have to differ");
    tokio::fs::write(&archive, &repacked).await.expect("the archive packed again");
    touch_later(&archive);

    let after_restart = harness::service(&pool, &dir, &google);
    let progress = std::sync::Arc::new(crate::backups::archive::Progress::default());
    after_restart
        .upload_archive(
            server,
            backup,
            &archive,
            repacked.len() as u64,
            "monday.tar.zst",
            &progress,
        )
        .await
        .expect("an upload of the new archive");

    let file = google.file_of_backup(backup).expect("the archive in her Drive");
    assert_eq!(
        file.bytes, repacked,
        "the backup in Drive is stitched out of two different archives, which is worse than \
         having none at all"
    );
    let _ = anna;
    let _ = dir;
}

#[tokio::test]
async fn how_far_an_upload_has_come_is_written_down_before_the_chunk_goes_out() {
    let stopped = stopped_after(1, Some(1)).await;
    let Interrupted { pool, google, backup, whole, .. } = stopped;

    let row = session_of(&pool, backup).await.expect("the session outlived the run");
    let chunk = super::upload::CHUNK;
    assert_eq!(
        row.offer(),
        Some((chunk, sha256_of(&whole[..chunk as usize]).as_str())),
        "Google was already holding a chunk that nothing on this machine could vouch for"
    );
    assert_eq!(google.chunks_seen(), 1, "the first chunk never even got an answer");
}

#[tokio::test]
async fn a_session_that_carries_no_mark_is_begun_again_rather_than_carried_on() {
    let stopped = an_upload_stopped_by_a_restart(2).await;
    let Interrupted { pool, dir, google, anna, server, backup, archive, whole } = stopped;
    sqlx::query("UPDATE drive_uploads SET offered_sha256 = NULL WHERE backup_id = ?")
        .bind(backup)
        .execute(&pool)
        .await
        .expect("a session of the kind the older panel left behind");
    let offered_by_then = google.bytes_offered();

    let after_restart = harness::service(&pool, &dir, &google);
    let progress = std::sync::Arc::new(crate::backups::archive::Progress::default());
    after_restart
        .upload_archive(server, backup, &archive, whole.len() as u64, "monday.tar.zst", &progress)
        .await
        .expect("an upload that cannot be proved is begun again, not given up on");

    assert_eq!(google.file_of_backup(backup).expect("the archive").bytes, whole);
    assert!(
        google.bytes_offered() >= offered_by_then + whole.len() as u64,
        "half an upload nothing vouches for was carried on anyway"
    );
    let _ = (anna, dir);
}

#[tokio::test]
async fn google_holding_more_than_the_mark_covers_is_begun_again_from_the_front() {
    let stopped = an_upload_stopped_by_a_restart(2).await;
    let Interrupted { pool, dir, google, anna, server, backup, archive, whole } = stopped;
    sqlx::query("UPDATE drive_uploads SET offered_bytes = 4096 WHERE backup_id = ?")
        .bind(backup)
        .execute(&pool)
        .await
        .expect("a mark that stayed behind because it could not be written");
    let offered_by_then = google.bytes_offered();

    let after_restart = harness::service(&pool, &dir, &google);
    let progress = std::sync::Arc::new(crate::backups::archive::Progress::default());
    after_restart
        .upload_archive(server, backup, &archive, whole.len() as u64, "monday.tar.zst", &progress)
        .await
        .expect("what cannot be proved is sent again");

    assert_eq!(google.file_of_backup(backup).expect("the archive").bytes, whole);
    assert!(
        google.bytes_offered() >= offered_by_then + whole.len() as u64,
        "the part beyond the mark was carried on although nothing covers it"
    );
    let _ = (anna, dir);
}

#[tokio::test]
async fn a_session_google_has_forgotten_starts_again_instead_of_failing() {
    let stopped = an_upload_stopped_by_a_restart(2).await;
    let Interrupted { pool, dir, google, anna, server, backup, archive, whole } = stopped;
    google.forget_every_session();

    let after_restart = harness::service(&pool, &dir, &google);
    let progress = std::sync::Arc::new(crate::backups::archive::Progress::default());
    after_restart
        .upload_archive(server, backup, &archive, whole.len() as u64, "monday.tar.zst", &progress)
        .await
        .expect("a session Google threw away is a reason to begin again, not to give up");

    assert_eq!(google.file_of_backup(backup).expect("the archive").bytes, whole);
    assert!(session_of(&pool, backup).await.is_none());
    assert!(!address_of(&dir, anna, backup).exists());
}

#[tokio::test]
async fn a_session_whose_archive_is_gone_is_worth_nothing() {
    let stopped = an_upload_stopped_by_a_restart(2).await;
    let Interrupted { pool, dir, google, anna, server, backup, archive, whole } = stopped;
    tokio::fs::remove_file(&archive).await.expect("the archive swept away");

    let after_restart = harness::service(&pool, &dir, &google);
    assert!(
        after_restart.resumable(backup, &archive, crate::model::Timestamp::now()).await.is_none(),
        "there is nothing left to carry on"
    );

    let progress = std::sync::Arc::new(crate::backups::archive::Progress::default());
    let refused = after_restart
        .upload_archive(server, backup, &archive, whole.len() as u64, "monday.tar.zst", &progress)
        .await
        .expect_err("nothing can be sent");
    assert!(matches!(refused, super::http::DriveError::Unreachable(_)), "{refused:?}");
    let _ = (anna, google);
}

#[tokio::test]
async fn an_upload_that_finished_before_the_restart_is_not_sent_a_second_time() {
    let stopped = an_upload_stopped_with_the_last_chunk_in_the_air().await;
    let Interrupted { pool, dir, google, anna, server, backup, archive, whole } = stopped;
    google.take_the_rest_quietly(&whole);
    let sent_by_then = google.chunks_seen();

    let after_restart = harness::service(&pool, &dir, &google);
    let progress = std::sync::Arc::new(crate::backups::archive::Progress::default());
    let stored = after_restart
        .upload_archive(server, backup, &archive, whole.len() as u64, "monday.tar.zst", &progress)
        .await
        .expect("Google had it all along");

    assert_eq!(
        google.chunks_seen(),
        sent_by_then,
        "asking first is what keeps a finished upload from being done all over again"
    );
    assert_eq!(google.file_of_backup(backup).expect("the archive").id, stored.file_id);
    assert!(session_of(&pool, backup).await.is_none());
    assert!(!address_of(&dir, anna, backup).exists());
}

#[tokio::test]
async fn two_runs_never_hold_the_same_session_at_once() {
    let (pool, dir, google, drive) = panel().await;
    harness::with_credentials(&drive).await;
    let anna = a_user(&pool, PanelRole::User).await;
    let server = a_server(&pool, anna).await;
    drive.of(anna).write_token("1//a-token").await;

    let whole = filler((super::upload::CHUNK + 4096) as usize, 3);
    tokio::fs::create_dir_all(dir.path()).await.expect("a place for the archive");
    let archive = dir.path().join("monday.tar.zst");
    tokio::fs::write(&archive, &whole).await.expect("an archive");
    let backup = a_drive_row(&pool, server, "Monday").await;
    google.hold_the_first_chunk(Duration::from_secs(3));

    let sending = start_sending(&drive, server, backup, &archive, whole.len() as u64);
    wait_for_chunks(&google, 1).await;

    let progress = std::sync::Arc::new(crate::backups::archive::Progress::default());
    let refused = drive
        .upload_archive(server, backup, &archive, whole.len() as u64, "monday.tar.zst", &progress)
        .await
        .expect_err("a second run must not write into the same session");
    assert!(matches!(refused, super::http::DriveError::Busy), "{refused:?}");
    sending.abort();
}

#[tokio::test]
async fn the_session_address_is_kept_like_a_key_and_shown_to_nobody() {
    let stopped = an_upload_stopped_by_a_restart(1).await;
    let Interrupted { pool, dir, google, anna, backup, .. } = stopped;

    let address = address_of(&dir, anna, backup);
    let written = tokio::fs::read_to_string(&address).await.expect("the address on the disk");
    assert!(written.contains("/upload/session/"), "that is not a session address: {written}");
    assert_eq!(mode_of(&address), 0o600, "the address itself");
    assert_eq!(mode_of(address.parent().expect("its directory")), 0o700, "its directory");

    let row: Option<String> = sqlx::query_scalar(
        "SELECT group_concat(quote(backup_id) || quote(user_id) || quote(total_bytes) || \
                quote(archive_mtime_ns) || quote(archive_inode) || quote(opened_at) || \
                quote(updated_at)) FROM drive_uploads",
    )
    .fetch_one(&pool)
    .await
    .expect("the row");
    let row = row.unwrap_or_default();
    assert!(
        !row.contains("http"),
        "a database that is itself copied into backups carries a key to somebody's Drive: {row}"
    );

    let drive = harness::service(&pool, &dir, &google);
    let status = serde_json::to_string(&drive.of(anna).status().await.expect("a status"))
        .expect("json");
    assert!(!status.contains("/upload/session/"), "the owner's own status leaks it: {status}");
    let overview =
        serde_json::to_string(&drive.admin_overview().await.expect("the overview")).expect("json");
    assert!(!overview.contains("/upload/session/"), "the admin overview leaks it: {overview}");
}

#[tokio::test]
async fn letting_go_of_an_account_lets_go_of_the_uploads_it_left_open() {
    let stopped = an_upload_stopped_by_a_restart(1).await;
    let Interrupted { pool, dir, google, anna, backup, .. } = stopped;

    let drive = harness::service(&pool, &dir, &google);
    drive.of(anna).disconnect(Files::Keep).await.expect("letting go");

    assert!(session_of(&pool, backup).await.is_none(), "the row went with the account");
    assert!(!address_of(&dir, anna, backup).exists(), "and so did the address");
}

#[tokio::test]
async fn a_session_past_googles_week_is_swept_and_an_address_without_a_row_with_it() {
    let stopped = an_upload_stopped_by_a_restart(1).await;
    let Interrupted { pool, dir, google, anna, server, backup, .. } = stopped;

    let stray = a_drive_row(&pool, server, "Tuesday").await;
    let address = address_of(&dir, anna, stray);
    tokio::fs::create_dir_all(address.parent().expect("the directory")).await.expect("a directory");
    tokio::fs::write(&address, "https://upload.example/session/stray").await.expect("a stray");

    let drive = harness::service(&pool, &dir, &google);
    let later = crate::model::Timestamp::at(
        crate::model::Timestamp::now().as_datetime() + time::Duration::days(7),
    );
    drive.sweep_sessions(later).await;

    assert!(session_of(&pool, backup).await.is_none(), "Google's week ran out");
    assert!(!address_of(&dir, anna, backup).exists());
    assert!(!address.exists(), "an address that belongs to no row is a key left lying about");
}

#[tokio::test]
async fn an_address_google_moves_is_the_one_kept_for_the_next_try() {
    let (pool, dir, google, drive) = panel().await;
    harness::with_credentials(&drive).await;
    let anna = a_user(&pool, PanelRole::User).await;
    let server = a_server(&pool, anna).await;
    drive.of(anna).write_token("1//a-token").await;

    let whole = filler((super::upload::CHUNK * 2 + 4096) as usize, 5);
    tokio::fs::create_dir_all(dir.path()).await.expect("a place for the archive");
    let archive = dir.path().join("monday.tar.zst");
    tokio::fs::write(&archive, &whole).await.expect("an archive on the disk");
    let backup = a_drive_row(&pool, server, "Monday").await;

    google.move_session_after(1);
    google.fail_chunk(2, 400);

    let progress = std::sync::Arc::new(crate::backups::archive::Progress::default());
    let stopped = drive
        .upload_archive(server, backup, &archive, whole.len() as u64, "monday.tar.zst", &progress)
        .await
        .expect_err("Google turned the second chunk away");
    assert!(
        matches!(stopped, super::http::DriveError::Refused { status: 400, .. }),
        "{stopped:?}"
    );

    let kept = tokio::fs::read_to_string(address_of(&dir, anna, backup))
        .await
        .expect("the address is still there for the next try");
    assert_eq!(
        google.chunks_seen(),
        2,
        "a 308 that carries a new address is the upload protocol speaking, not a redirect for          the HTTP client to follow behind our back"
    );
    assert!(
        kept.contains("/upload/session/moved-"),
        "the next try would knock at a door Google has already moved away from: {kept}"
    );
    assert!(session_of(&pool, backup).await.is_some(), "and the row that goes with it");
}
