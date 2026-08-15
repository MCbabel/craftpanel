use std::time::Duration;

use crate::model::{
    BackupLocation, BackupTargetPolicy, BackupTargetReason, DriveAccountState, DriveFileState,
    DriveLinkState, Id, PanelRole,
};
use crate::ops::testing::{a_server, a_user, schema};

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
