use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use sha1::Sha1;
use sha2::{Digest, Sha512};
use sqlx::SqlitePool;

use super::harness::{self, DataDir, FakeModrinth};
use super::modrinth::{a_version, MrDependency, MrFile, MrHashes, MrVersion};
use super::store;
use super::types::*;
use super::{Content, PackSource};
use crate::auth::access::Access;
use crate::auth::Disks;
use crate::helper::Helper;
use crate::model::{
    ContentProjectType, ContentSourceKind, Id, OperationState, Permission, Permissions, Timestamp,
};
use crate::ops::Operations;

struct Panel {
    pool: SqlitePool,
    loader: String,
    published: std::cell::Cell<u32>,
    content: Arc<Content>,
    operations: Arc<Operations>,
    upstream: FakeModrinth,
    helper: crate::auth::harness::FakeHelper,
    owner: Id,
    server: Id,
    root: PathBuf,
    _dir: DataDir,
}

impl Panel {
    async fn new(loader: &str, game_version: &str) -> Self {
        let pool = harness::schema().await;
        let dir = DataDir::new();
        let owner = harness::a_user(&pool).await;
        let server = harness::a_server(&pool, owner, loader, game_version).await;

        let root = dir
            .path()
            .join("users")
            .join(owner.to_string())
            .join("servers")
            .join(server.to_string());
        std::fs::create_dir_all(&root).expect("a server directory");

        let upstream = harness::fake_modrinth().await;
        let helper = crate::auth::harness::FakeHelper::obliging()
            .await
            .rooted_at(dir.path().join("users"));
        let operations = Operations::new(pool.clone(), dir.path());
        let content = Content::with_modrinth(
            pool.clone(),
            dir.path(),
            Helper::new(helper.socket()),
            Arc::clone(&operations),
            Arc::new(harness::client(&pool, &upstream)),
            Disks::none(),
        );

        Self {
            pool,
            loader: loader.to_owned(),
            published: std::cell::Cell::new(0),
            content,
            operations,
            upstream,
            helper,
            owner,
            server,
            root,
            _dir: dir,
        }
    }

    fn write(&self, relative: &str, body: &[u8]) {
        let path = self.root.join(relative);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("a directory");
        std::fs::write(path, body).expect("a file");
    }

    fn exists(&self, relative: &str) -> bool {
        self.root.join(relative).exists()
    }

    fn access(&self) -> Access {
        Access {
            server_id: self.server,
            owner_id: self.owner,
            permissions: Permissions::of(Permission::ServerAdmin),
        }
    }

    async fn listed(&self) -> ContentListResponse {
        self.content.list(self.access(), false).await.expect("a list")
    }

    async fn row(&self, id: Id) -> Option<store::ItemRow> {
        store::one(&self.pool, self.server, id).await.expect("a read")
    }

    fn chowned(&self) -> usize {
        self.helper
            .calls()
            .iter()
            .filter(|call| matches!(call, craftpanel_proto::HelperRequest::ChownTree { .. }))
            .count()
    }

    async fn settled(&self, operation: Id) -> crate::model::Operation {
        for _ in 0..200 {
            let run = self.operations.get(operation).await.expect("the operation");
            if run.state.is_terminal() {
                return run;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("the run never ended");
    }

    fn publish(&self, project: &str, version: &str, file_name: &str, body: &[u8]) -> MrVersion {
        let url = self.upstream.add_file(file_name, body.to_vec());
        let day = self.published.get() + 1;
        self.published.set(day);
        let mut published =
            a_version(version, project, "release", &format!("2026-06-{day:02}T00:00:00Z"));
        published.loaders = vec![self.loader.clone()];
        published.files = vec![MrFile {
            hashes: MrHashes {
                sha1: Some(hex::encode(sha1::Digest::finalize(<Sha1 as sha1::Digest>::new_with_prefix(body)))),
                sha512: Some(hex::encode(Sha512::digest(body))),
            },
            url,
            filename: file_name.to_owned(),
            primary: true,
            size: body.len() as u64,
        }];
        published
    }
}

#[tokio::test]
async fn the_list_is_the_disk_and_the_database_put_together() {
    let panel = Panel::new("paper", "1.21.1").await;
    panel.write("plugins/one.jar", b"a plugin");
    panel.write("plugins/two.jar.disabled", b"another plugin");
    panel.write("plugins/notes.txt", b"not content");

    let listed = panel.listed().await;
    assert_eq!(listed.content_type, ContentProjectType::Plugin, "8.1: Paper is a plugin platform");
    assert_eq!(listed.game_version, "1.21.1");
    assert!(listed.permissions.can_write);
    assert!(!listed.truncated);

    let names: Vec<&str> = listed.items.iter().map(|item| item.file_name.as_str()).collect();
    assert_eq!(names, ["one.jar", "two.jar.disabled"]);
    assert!(listed.items[0].enabled);
    assert!(!listed.items[1].enabled);
    assert_eq!(listed.items[0].file_path, "/plugins/one.jar", "7.1: answers lead with a slash");
    assert!(listed.items[0].project.is_none(), "a file we never installed has no project");
    assert!(listed.modpack.is_none());
}

#[tokio::test]
async fn a_row_id_survives_a_second_look_and_a_rename() {
    let panel = Panel::new("fabric", "1.21.1").await;
    panel.write("mods/keep.jar", b"jar");

    let first = panel.listed().await.items[0].id;
    let again = panel.listed().await.items[0].id;
    assert_eq!(first, again, "8.1: the selection in the browser hangs on this id");

    panel.content.set_enabled(panel.server, &[first], false).await.expect("a disable");
    let after = panel.listed().await;
    assert_eq!(after.items[0].id, first);
    assert_eq!(after.items[0].file_name, "keep.jar.disabled");
}

#[tokio::test]
async fn a_viewer_is_told_they_may_not_write() {
    let panel = Panel::new("fabric", "1.21.1").await;
    let viewer = Access {
        server_id: panel.server,
        owner_id: panel.owner,
        permissions: Permissions::from_role(crate::model::ServerRole::Viewer),
    };
    let listed = panel.content.list(viewer, false).await.expect("a list");
    assert!(listed.permissions.can_read);
    assert!(!listed.permissions.can_write, "2.1: content writing is SETUP");
}

#[tokio::test]
async fn disabling_renames_the_file_and_hands_the_tree_back() {
    let panel = Panel::new("fabric", "1.21.1").await;
    panel.write("mods/foo.jar", b"jar");
    let id = panel.listed().await.items[0].id;

    let answer = panel.content.set_enabled(panel.server, &[id], false).await.expect("a disable");
    assert!(answer.results[0].ok);
    assert_eq!(answer.results[0].file_path.as_deref(), Some("/mods/foo.jar.disabled"));
    assert_eq!(answer.results[0].enabled, Some(false));

    assert!(!panel.exists("mods/foo.jar"));
    assert!(panel.exists("mods/foo.jar.disabled"));
    assert_eq!(panel.chowned(), 1, "the game process must own its own mod again");

    let chown = panel
        .helper
        .calls()
        .into_iter()
        .find_map(|call| match call {
            craftpanel_proto::HelperRequest::ChownTree { user_id, steps } => Some((user_id, steps)),
            _ => None,
        })
        .expect("a chown");
    assert_eq!(chown.0, panel.owner.to_string());
    assert_eq!(chown.1, crate::helper::in_servers(panel.server));
}

#[tokio::test]
async fn enabling_takes_the_suffix_off_again() {
    let panel = Panel::new("fabric", "1.21.1").await;
    panel.write("mods/foo.jar.disabled", b"jar");
    let id = panel.listed().await.items[0].id;

    panel.content.set_enabled(panel.server, &[id], true).await.expect("an enable");
    assert!(panel.exists("mods/foo.jar"));
    assert!(!panel.exists("mods/foo.jar.disabled"));
    assert!(panel.row(id).await.expect("the row").enabled);
}

#[tokio::test]
async fn a_batch_reports_each_item_and_does_not_throw_away_the_ones_that_worked() {
    let panel = Panel::new("fabric", "1.21.1").await;
    panel.write("mods/here.jar", b"jar");
    let good = panel.listed().await.items[0].id;
    let missing = Id::new();

    let answer =
        panel.content.delete(panel.server, &[good, missing]).await.expect("a mixed batch");
    assert_eq!(answer.results.len(), 2);
    assert!(answer.results[0].ok);
    assert!(answer.results[0].file_name.is_none(), "8.5: a deleted file has nothing to report");
    assert!(!answer.results[1].ok);
    assert_eq!(answer.results[1].error.as_deref(), Some("content_not_found"));

    assert!(!panel.exists("mods/here.jar"));
    assert!(panel.row(good).await.is_none());
}

#[tokio::test]
async fn deleting_a_folder_shaped_item_hands_it_back_before_it_walks_into_it() {
    let panel = Panel::new("paper", "1.21.1").await;
    panel.write("plugins/WorldEdit/lang/strings.json", b"{}");
    panel.write("plugins/one.jar", b"jar");

    let jar = panel
        .listed()
        .await
        .items
        .iter()
        .find(|item| item.file_name == "one.jar")
        .expect("the jar")
        .id;
    let folder =
        store::ItemRow::fresh(panel.server, "plugins/WorldEdit", ContentProjectType::Plugin);
    store::upsert(&panel.pool, &folder).await.expect("a row for the folder");

    let answer = panel.content.delete(panel.server, &[folder.id]).await.expect("a delete");
    assert!(answer.results[0].ok, "{:?}", answer.results[0]);
    assert!(!panel.exists("plugins/WorldEdit"));

    let handed: Vec<Vec<String>> = panel
        .helper
        .calls()
        .into_iter()
        .filter_map(|call| match call {
            craftpanel_proto::HelperRequest::ChownTree { steps, .. } => Some(steps),
            _ => None,
        })
        .collect();
    let folder = ["plugins".to_owned(), "WorldEdit".to_owned()];
    assert_eq!(
        handed,
        vec![crate::helper::below_server(panel.server, &folder)],
        "the folder, and only it"
    );

    let before = panel.chowned();
    assert!(panel.content.delete(panel.server, &[jar]).await.expect("a delete").results[0].ok);
    assert_eq!(panel.chowned(), before, "a file is unlinked out of its parent, no walk, no call");
}

#[tokio::test]
async fn deleting_a_row_that_names_a_link_takes_the_link_and_not_its_target() {
    let panel = Panel::new("fabric", "1.21.1").await;
    std::fs::create_dir_all(panel.root.join("mods")).expect("a directory");
    panel.write("world/level.dat", b"the world");
    std::os::unix::fs::symlink(
        panel.root.join("world").join("level.dat"),
        panel.root.join("mods").join("sneaky.jar"),
    )
    .expect("a link");
    assert!(panel.listed().await.items.is_empty(), "a link is not a mod");

    let row = store::ItemRow::fresh(panel.server, "mods/sneaky.jar", ContentProjectType::Mod);
    store::upsert(&panel.pool, &row).await.expect("a row that names the link");

    let answer = panel.content.delete(panel.server, &[row.id]).await.expect("a delete");
    assert!(answer.results[0].ok);
    assert!(!panel.exists("mods/sneaky.jar"));
    assert!(panel.exists("world/level.dat"), "the target is not ours to remove");
}

#[tokio::test]
async fn installing_from_modrinth_lands_the_jar_checks_it_and_writes_down_where_it_came_from() {
    let panel = Panel::new("fabric", "1.21.1").await;
    let version = panel.publish("MOD", "v1", "shiny-1.0.jar", b"the bytes of shiny 1.0");
    panel.upstream.set_versions("MOD", vec![version]);

    let answer = panel
        .content
        .install(
            panel.server,
            &ContentInstallRequest {
                items: vec![ContentInstallTarget {
                    project_id: "MOD".to_owned(),
                    version_id: None,
                }],
                resolve_dependencies: true,
            },
            Some(panel.owner),
        )
        .await
        .expect("an accepted install");

    assert_eq!(answer.planned.len(), 1);
    assert_eq!(answer.planned[0].file_name, "shiny-1.0.jar");
    assert_eq!(answer.operation.state, OperationState::Queued);

    let run = panel.settled(answer.operation.id).await;
    assert_eq!(run.state, OperationState::Done, "{:?}", run.error);
    assert_eq!(
        std::fs::read(panel.root.join("mods").join("shiny-1.0.jar")).expect("the jar"),
        b"the bytes of shiny 1.0"
    );
    assert_eq!(panel.chowned(), 1, "one call per run, not per file");

    let listed = panel.listed().await;
    assert_eq!(listed.items.len(), 1);
    assert_eq!(listed.items[0].source_kind, ContentSourceKind::ServerProject);
    let version = listed.items[0].version.as_ref().expect("a version to show");
    assert_eq!(version.id, "v1");
    assert_eq!(version.version_number, "v1", "the number the card shows, not the id");
    assert!(version.date_published.is_some(), "the card shows when it came out");
    let row = panel.row(listed.items[0].id).await.expect("the row");
    assert_eq!(row.project_id.as_deref(), Some("MOD"));
    assert_eq!(row.version_id.as_deref(), Some("v1"));
}

#[tokio::test]
async fn a_file_that_does_not_match_its_checksum_never_reaches_the_mods_directory() {
    let panel = Panel::new("fabric", "1.21.1").await;
    let mut version = panel.publish("MOD", "v1", "shiny-1.0.jar", b"the real bytes");
    version.files[0].hashes.sha512 = Some("00".repeat(64));
    panel.upstream.set_versions("MOD", vec![version]);

    let answer = panel
        .content
        .install(
            panel.server,
            &ContentInstallRequest {
                items: vec![ContentInstallTarget {
                    project_id: "MOD".to_owned(),
                    version_id: None,
                }],
                resolve_dependencies: false,
            },
            Some(panel.owner),
        )
        .await
        .expect("an accepted install");

    let run = panel.settled(answer.operation.id).await;
    assert_eq!(run.state, OperationState::Failed);
    assert_eq!(run.error.expect("an error").code, "checksum_mismatch");
    assert!(!panel.exists("mods/shiny-1.0.jar"));
    assert!(panel.listed().await.items.is_empty());
}

#[tokio::test]
async fn a_plugin_platform_installs_into_plugins_and_a_modloader_into_mods() {
    for (loader, directory) in [("paper", "plugins"), ("fabric", "mods")] {
        let panel = Panel::new(loader, "1.21.1").await;
        let version = panel.publish("MOD", "v1", "thing.jar", b"bytes");
        panel.upstream.set_versions("MOD", vec![version]);

        let answer = panel
            .content
            .install(
                panel.server,
                &ContentInstallRequest {
                    items: vec![ContentInstallTarget {
                        project_id: "MOD".to_owned(),
                        version_id: None,
                    }],
                    resolve_dependencies: false,
                },
                None,
            )
            .await
            .expect("an accepted install");
        panel.settled(answer.operation.id).await;
        assert!(panel.exists(&format!("{directory}/thing.jar")), "{loader}");
    }
}

#[tokio::test]
async fn installing_nothing_that_fits_is_an_http_failure_and_not_a_run() {
    let panel = Panel::new("paper", "1.21.1").await;
    let mut fabric_only = a_version("v1", "MOD", "release", "2026-06-01T00:00:00Z");
    fabric_only.loaders = vec!["fabric".to_owned()];
    panel.upstream.set_versions("MOD", vec![fabric_only]);

    let refusal = panel
        .content
        .install(
            panel.server,
            &ContentInstallRequest {
                items: vec![ContentInstallTarget {
                    project_id: "MOD".to_owned(),
                    version_id: None,
                }],
                resolve_dependencies: true,
            },
            None,
        )
        .await
        .expect_err("nothing fits");
    assert_eq!(refusal.code(), "no_compatible_version");
    assert_eq!(refusal.status(), axum::http::StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn an_update_replaces_the_file_and_keeps_the_row_the_selection_holds() {
    let panel = Panel::new("fabric", "1.21.1").await;
    let old = panel.publish("MOD", "v1", "shiny-1.0.jar", b"version one");
    let new = panel.publish("MOD", "v2", "shiny-2.0.jar", b"version two");
    panel.upstream.set_versions("MOD", vec![old, new]);

    let installed = panel
        .content
        .install(
            panel.server,
            &ContentInstallRequest {
                items: vec![ContentInstallTarget {
                    project_id: "MOD".to_owned(),
                    version_id: Some("v1".to_owned()),
                }],
                resolve_dependencies: false,
            },
            None,
        )
        .await
        .expect("an install");
    panel.settled(installed.operation.id).await;
    let id = panel.listed().await.items[0].id;

    let update = panel
        .content
        .update(
            panel.server,
            &ContentUpdateRequest { items: Vec::new(), all: true },
            Some(panel.owner),
        )
        .await
        .expect("an accepted update");
    assert_eq!(update.total, 1, "8.6: the denominator of the progress bar");

    let run = panel.settled(update.operation.id).await;
    assert_eq!(run.state, OperationState::Done, "{:?}", run.error);
    assert!(panel.exists("mods/shiny-2.0.jar"));
    assert!(!panel.exists("mods/shiny-1.0.jar"), "the old file goes with the update");

    let after = panel.listed().await;
    assert_eq!(after.items.len(), 1);
    assert_eq!(after.items[0].id, id, "the row survives, or the selection is thrown away");
    assert_eq!(panel.row(id).await.expect("the row").version_id.as_deref(), Some("v2"));
}

#[tokio::test]
async fn an_update_with_nothing_newer_is_refused_before_a_run_is_opened() {
    let panel = Panel::new("fabric", "1.21.1").await;
    panel.write("mods/local.jar", b"never came from Modrinth");
    panel.listed().await;

    let refusal = panel
        .content
        .update(panel.server, &ContentUpdateRequest { items: Vec::new(), all: true }, None)
        .await
        .expect_err("nothing to do");
    assert_eq!(refusal.code(), "no_compatible_version");
    assert!(panel.operations.snapshot(panel.server).await.expect("a snapshot").operations.is_empty());
}

#[tokio::test]
async fn the_update_check_marks_what_has_a_newer_version_and_locks_nothing() {
    let panel = Panel::new("fabric", "1.21.1").await;
    let old = panel.publish("MOD", "v1", "shiny-1.0.jar", b"one");
    panel.upstream.set_versions("MOD", vec![old.clone()]);

    let installed = panel
        .content
        .install(
            panel.server,
            &ContentInstallRequest {
                items: vec![ContentInstallTarget {
                    project_id: "MOD".to_owned(),
                    version_id: None,
                }],
                resolve_dependencies: false,
            },
            None,
        )
        .await
        .expect("an install");
    panel.settled(installed.operation.id).await;

    let newer = panel.publish("MOD", "v2", "shiny-2.0.jar", b"two");
    panel.upstream.set_versions("MOD", vec![old, newer]);
    sqlx::query("UPDATE modrinth_project_versions SET expires_at = '2000-01-01T00:00:00Z'")
        .execute(&panel.pool)
        .await
        .expect("an expired cache");

    panel.content.check_updates(panel.server).await.expect("a check");
    assert!(
        panel.operations.busy_reasons(panel.server).await.expect("reasons").is_empty(),
        "8.16: the check sets no lock, or a delete would fail at random"
    );

    let listed = panel.listed().await;
    assert!(listed.items[0].has_update);
    assert_eq!(listed.items[0].update_version_id.as_deref(), Some("v2"));
    assert!(listed.updates_checked_at.is_some());
}

#[tokio::test]
async fn a_finished_install_asks_again_about_what_it_touched_and_leaves_the_rest_alone() {
    let panel = Panel::new("fabric", "1.21.1").await;

    panel.write("mods/other-1.0.jar", b"other one");
    let scanned = super::scan::reconcile(&panel.pool, panel.server, &panel.root)
        .await
        .expect("a scan");
    let mut other = scanned.into_iter().next().expect("the row of the file");
    other.project_id = Some("OTHER".to_owned());
    other.version_id = Some("o1".to_owned());
    store::upsert(&panel.pool, &other).await.expect("an origin for it");
    let first = panel.publish("OTHER", "o1", "other-1.0.jar", b"other one");
    let later = panel.publish("OTHER", "o2", "other-2.0.jar", b"other two");
    panel.upstream.set_versions("OTHER", vec![first, later]);

    let old = panel.publish("MOD", "v1", "shiny-1.0.jar", b"one");
    let new = panel.publish("MOD", "v2", "shiny-2.0.jar", b"two");
    panel.upstream.set_versions("MOD", vec![old, new]);

    let pinned = panel
        .content
        .install(panel.server, &wants("MOD", Some("v1")), None)
        .await
        .expect("an install of the version the user picked");
    let run = panel.settled(pinned.operation.id).await;
    assert_eq!(run.state, OperationState::Done, "{:?}", run.error);

    let mut touched = None;
    for _ in 0..200 {
        let found = of_project(&panel, "MOD").await;
        if found.as_ref().is_some_and(|row| row.has_update) {
            touched = found;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let touched = touched.expect("8.16: the project the run touched is asked about again");
    assert_eq!(touched.update_version_id.as_deref(), Some("v2"));

    let other = of_project(&panel, "OTHER").await.expect("the other row");
    assert!(
        !other.has_update,
        "8.16: only the projects the run touched, and o2 was published for this one"
    );
    let checked: Option<Timestamp> =
        sqlx::query_scalar("SELECT updates_checked_at FROM servers WHERE id = ?")
            .bind(panel.server)
            .fetch_one(&panel.pool)
            .await
            .expect("the server row");
    assert!(checked.is_none(), "two projects are not the whole server");
}

#[tokio::test]
async fn every_row_a_run_wrote_carries_the_project_its_update_button_needs() {
    let panel = Panel::new("paper", "1.21.1").await;
    let mut version = panel.publish("MOD", "v1", "worldedit-bukkit-7.4.3.jar", b"a plugin");
    version.dependencies = vec![MrDependency {
        project_id: Some("DEP".to_owned()),
        version_id: None,
        dependency_type: "required".to_owned(),
    }];
    panel.upstream.set_versions("MOD", vec![version]);
    let library = panel.publish("DEP", "d1", "worldedit-libs.jar", b"a library");
    panel.upstream.set_versions("DEP", vec![library]);
    panel.upstream.set_project(
        "MOD",
        serde_json::json!({
            "id": "MOD",
            "slug": "worldedit",
            "title": "WorldEdit",
            "icon_url": "https://cdn.invalid/worldedit.png",
            "project_type": "plugin"
        }),
    );

    let answer = panel
        .content
        .install(
            panel.server,
            &ContentInstallRequest {
                items: vec![ContentInstallTarget {
                    project_id: "MOD".to_owned(),
                    version_id: None,
                }],
                resolve_dependencies: true,
            },
            None,
        )
        .await
        .expect("an install");
    let run = panel.settled(answer.operation.id).await;
    assert_eq!(run.state, OperationState::Done, "{:?}", run.error);

    let listed = panel.listed().await;
    let asked_for = listed
        .items
        .iter()
        .find(|item| item.file_name == "worldedit-bukkit-7.4.3.jar")
        .expect("the plugin");
    let project = asked_for
        .project
        .as_ref()
        .expect("8.1: with no project the update dialog cannot open");
    assert_eq!(project.id, "MOD");
    assert_eq!(project.title, "WorldEdit", "or the row is named after the file it landed as");
    assert_eq!(project.slug.as_deref(), Some("worldedit"));
    assert_eq!(project.icon_url.as_deref(), Some("https://cdn.invalid/worldedit.png"));

    let dependency = listed
        .items
        .iter()
        .find(|item| item.file_name == "worldedit-libs.jar")
        .expect("what it needs came with it");
    assert_eq!(
        dependency.project.as_ref().expect("a dependency is a row like any other").id,
        "DEP",
        "the request never named it, so only the run can have fetched it"
    );
}

#[tokio::test]
async fn the_background_check_catches_up_a_row_that_never_had_a_project() {
    let panel = Panel::new("paper", "1.21.1").await;
    panel.write("plugins/worldedit-bukkit-7.4.3.jar", b"a plugin");
    let installed = panel.publish("MOD", "v1", "worldedit-bukkit-7.4.3.jar", b"a plugin");
    panel.upstream.set_versions("MOD", vec![installed]);
    panel.upstream.set_project(
        "MOD",
        serde_json::json!({ "id": "MOD", "slug": "worldedit", "title": "WorldEdit" }),
    );

    let scanned = super::scan::reconcile(&panel.pool, panel.server, &panel.root)
        .await
        .expect("a scan");
    let mut row = scanned.into_iter().next().expect("the row of the file");
    row.project_id = Some("MOD".to_owned());
    row.version_id = Some("v1".to_owned());
    store::upsert(&panel.pool, &row).await.expect("an origin for it");
    assert!(
        panel.content.modrinth().cached_project("MOD").await.expect("a read").is_none(),
        "the gap this closes"
    );

    touch(&panel, panel.server, Timestamp::now(), Some(ago(7 * 60 * 60))).await;
    let live = crate::auth::LiveServers::fixed([]);
    let swept = panel.content.sweep_once(&live).await.expect("a pass");
    assert!(swept.contains(&panel.server), "seven hours are more than six");

    let listed = panel.listed().await;
    let project = listed.items[0]
        .project
        .as_ref()
        .expect("the check has to bring the project along, or the row stays a file name");
    assert_eq!(project.id, "MOD");
    assert_eq!(project.title, "WorldEdit");
}

#[tokio::test]
async fn a_row_names_its_project_even_when_nothing_is_cached() {
    let panel = Panel::new("paper", "1.21.1").await;
    panel.write("plugins/worldedit-bukkit-7.4.3.jar", b"a plugin");

    let scanned = super::scan::reconcile(&panel.pool, panel.server, &panel.root)
        .await
        .expect("a scan");
    let mut row = scanned.into_iter().next().expect("the row of the file");
    row.project_id = Some("MOD".to_owned());
    row.version_id = Some("v1".to_owned());
    store::upsert(&panel.pool, &row).await.expect("an origin for it");

    let listed = panel.listed().await;
    assert!(
        panel.content.modrinth().cached_project("MOD").await.expect("a read").is_none(),
        "nothing published it, so the cache cannot have it"
    );
    assert!(listed.items[0].project.is_none(), "the card is what is missing");
    assert_eq!(
        listed.items[0].project_id.as_deref(),
        Some("MOD"),
        "8.1: without it the update button has nothing to ask Modrinth for"
    );
}

#[tokio::test]
async fn a_datapack_installs_because_the_plan_is_told_it_is_a_datapack() {
    let panel = Panel::new("vanilla", "1.21.1").await;
    let mut version = panel.publish("PACK", "d1", "terralith.zip", b"a datapack");
    version.loaders = vec!["datapack".to_owned()];
    panel.upstream.set_versions("PACK", vec![version]);
    panel.upstream.set_project(
        "PACK",
        serde_json::json!({
            "id": "PACK",
            "slug": "terralith",
            "title": "Terralith",
            "project_type": "datapack"
        }),
    );

    let answer = panel
        .content
        .install(panel.server, &wants("PACK", None), None)
        .await
        .expect("8.7: a datapack fits a vanilla server");
    let run = panel.settled(answer.operation.id).await;
    assert_eq!(run.state, OperationState::Done, "{:?}", run.error);
    assert!(panel.exists("world/datapacks/terralith.zip"));
}

#[tokio::test]
async fn the_background_sweep_takes_the_servers_that_are_due_and_leaves_the_rest() {
    let panel = Panel::new("fabric", "1.21.1").await;
    let a_month = ago(30 * 24 * 60 * 60);
    let ten_minutes = ago(10 * 60);

    let running = harness::a_server(&panel.pool, panel.owner, "fabric", "1.21.1").await;
    let forgotten = harness::a_server(&panel.pool, panel.owner, "fabric", "1.21.1").await;
    let just_checked = harness::a_server(&panel.pool, panel.owner, "fabric", "1.21.1").await;
    touch(&panel, running, a_month, None).await;
    touch(&panel, forgotten, a_month, None).await;
    touch(&panel, just_checked, Timestamp::now(), Some(ten_minutes)).await;

    let live = crate::auth::LiveServers::fixed([running]);
    let swept = panel.content.sweep_once(&live).await.expect("a pass");

    assert!(swept.contains(&panel.server), "used just now and never checked");
    assert!(swept.contains(&running), "a running server is checked however long it sat idle");
    assert!(!swept.contains(&forgotten), "nobody has opened it in a month");
    assert!(!swept.contains(&just_checked), "checked ten minutes ago; six hours are not up");
    assert_eq!(swept.len(), 2);
}

fn wants(project: &str, version: Option<&str>) -> ContentInstallRequest {
    ContentInstallRequest {
        items: vec![ContentInstallTarget {
            project_id: project.to_owned(),
            version_id: version.map(str::to_owned),
        }],
        resolve_dependencies: false,
    }
}

async fn of_project(panel: &Panel, project: &str) -> Option<store::ItemRow> {
    store::list(&panel.pool, panel.server)
        .await
        .expect("the rows")
        .into_iter()
        .find(|row| row.project_id.as_deref() == Some(project))
}

fn ago(seconds: u64) -> Timestamp {
    Timestamp::at(Timestamp::now().as_datetime() - Duration::from_secs(seconds))
}

async fn touch(panel: &Panel, server: Id, updated: Timestamp, checked: Option<Timestamp>) {
    sqlx::query("UPDATE servers SET updated_at = ?, updates_checked_at = ? WHERE id = ?")
        .bind(updated)
        .bind(checked)
        .bind(server)
        .execute(&panel.pool)
        .await
        .expect("a server");
}

fn a_pack(entries: &[(&str, &[u8])]) -> PathBuf {
    use std::io::Write;
    let path = std::env::temp_dir().join(format!("craftpanel-test-{}.mrpack", Id::new()));
    let mut writer = zip::ZipWriter::new(std::fs::File::create(&path).expect("a file"));
    let options: zip::write::FileOptions<'_, ()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, body) in entries {
        writer.start_file(*name, options).expect("an entry");
        writer.write_all(body).expect("the body");
    }
    writer.finish().expect("a finished archive");
    path
}

fn index(files: serde_json::Value) -> Vec<u8> {
    serde_json::json!({
        "formatVersion": 1,
        "game": "minecraft",
        "versionId": "2.0",
        "name": "A Small Pack",
        "files": files,
        "dependencies": { "minecraft": "1.21.1", "fabric-loader": "0.16.9" }
    })
    .to_string()
    .into_bytes()
}

#[tokio::test]
async fn an_uploaded_pack_lays_out_its_files_and_its_server_overrides() {
    let panel = Panel::new("fabric", "1.21.1").await;
    let body = b"a packed mod";
    let url = panel.upstream.add_file("packed.jar", body.to_vec());
    let archive = a_pack(&[
        (
            "modrinth.index.json",
            &index(serde_json::json!([{
                "path": "mods/packed.jar",
                "downloads": [url],
                "hashes": { "sha512": hex::encode(Sha512::digest(body)) },
                "fileSize": body.len()
            }])),
        ),
        ("overrides/config/thing.toml", b"from overrides"),
        ("server-overrides/config/thing.toml", b"from server overrides"),
        ("client-overrides/options.txt", b"never"),
    ]);

    let operation = panel
        .content
        .install_modpack(
            panel.server,
            PackSource::Upload { archive: archive.clone(), file_name: "small.mrpack".to_owned() },
            false,
            Some(panel.owner),
            false,
        )
        .await
        .expect("an accepted install");

    let run = panel.settled(operation.id).await;
    assert_eq!(run.state, OperationState::Done, "{:?}", run.error);
    assert_eq!(std::fs::read(panel.root.join("mods").join("packed.jar")).expect("the jar"), body);
    assert_eq!(
        std::fs::read(panel.root.join("config").join("thing.toml")).expect("the config"),
        b"from server overrides"
    );
    assert!(!panel.exists("options.txt"), "8.17: client overrides are dropped");
    assert!(panel.chowned() >= 1);

    let listed = panel.listed().await;
    assert!(listed.items.is_empty(), "8.1: pack files are not in the main list");
    let pack = listed.modpack.expect("a linked pack");
    assert_eq!(pack.title, "A Small Pack");
    assert_eq!(pack.filename.as_deref(), Some("small.mrpack"));

    let inside = panel.content.modpack_contents(panel.server).await.expect("the pack contents");
    assert_eq!(inside.items.len(), 1);
    assert_eq!(inside.items[0].file_name, "packed.jar");

    let _ = std::fs::remove_file(&archive);
}

#[tokio::test]
async fn a_pack_that_names_a_path_outside_the_server_writes_nothing_at_all() {
    let panel = Panel::new("fabric", "1.21.1").await;
    let body = b"malice";
    let url = panel.upstream.add_file("evil.jar", body.to_vec());
    let archive = a_pack(&[(
        "modrinth.index.json",
        &index(serde_json::json!([{
            "path": "../../../../escaped.jar",
            "downloads": [url],
            "hashes": { "sha512": hex::encode(Sha512::digest(body)) }
        }])),
    )]);

    let operation = panel
        .content
        .install_modpack(
            panel.server,
            PackSource::Upload { archive: archive.clone(), file_name: "evil.mrpack".to_owned() },
            false,
            None,
            false,
        )
        .await
        .expect("an accepted install");

    let run = panel.settled(operation.id).await;
    assert_eq!(run.state, OperationState::Failed);
    assert_eq!(run.error.expect("an error").code, "invalid_modpack");

    let above = panel.root.join("..").join("..").join("..").join("..").join("escaped.jar");
    assert!(!above.exists(), "the pack must not be able to name anything outside");
    assert!(store::modpack(&panel.pool, panel.server).await.expect("a read").is_none());
    let _ = std::fs::remove_file(&archive);
}

#[tokio::test]
async fn a_client_only_mod_in_a_pack_is_not_laid_out_on_a_server() {
    let panel = Panel::new("fabric", "1.21.1").await;
    let body = b"server side";
    let url = panel.upstream.add_file("server.jar", body.to_vec());
    let archive = a_pack(&[(
        "modrinth.index.json",
        &index(serde_json::json!([
            {
                "path": "mods/server.jar",
                "downloads": [url],
                "hashes": { "sha512": hex::encode(Sha512::digest(body)) },
                "env": { "client": "required", "server": "required" }
            },
            {
                "path": "mods/client.jar",
                "downloads": ["https://cdn.modrinth.com/data/AANobbMI/versions/x/client.jar"],
                "hashes": { "sha512": "ab" },
                "env": { "client": "required", "server": "unsupported" }
            }
        ])),
    )]);

    let operation = panel
        .content
        .install_modpack(
            panel.server,
            PackSource::Upload { archive: archive.clone(), file_name: "pack.mrpack".to_owned() },
            false,
            None,
            false,
        )
        .await
        .expect("an accepted install");
    let run = panel.settled(operation.id).await;
    assert_eq!(run.state, OperationState::Done, "{:?}", run.error);

    assert!(panel.exists("mods/server.jar"));
    assert!(!panel.exists("mods/client.jar"), "8.17: a server has no use for a client mod");
    let _ = std::fs::remove_file(&archive);
}

#[tokio::test]
async fn unlinking_leaves_the_files_and_moves_them_into_the_main_list() {
    let panel = Panel::new("fabric", "1.21.1").await;
    let body = b"a packed mod";
    let url = panel.upstream.add_file("packed.jar", body.to_vec());
    let archive = a_pack(&[(
        "modrinth.index.json",
        &index(serde_json::json!([{
            "path": "mods/packed.jar",
            "downloads": [url],
            "hashes": { "sha512": hex::encode(Sha512::digest(body)) }
        }])),
    )]);
    let operation = panel
        .content
        .install_modpack(
            panel.server,
            PackSource::Upload { archive: archive.clone(), file_name: "p.mrpack".to_owned() },
            false,
            None,
            false,
        )
        .await
        .expect("an install");
    panel.settled(operation.id).await;

    let answer = panel.content.unlink_modpack(panel.server).await.expect("an unlink");
    assert!(answer.unlinked);
    assert_eq!(answer.adopted_items, 1);
    assert!(panel.exists("mods/packed.jar"), "8.12: the files stay");

    let listed = panel.listed().await;
    assert!(listed.modpack.is_none());
    assert_eq!(listed.items.len(), 1);
    assert_eq!(listed.items[0].source_kind, ContentSourceKind::Local);

    assert_eq!(
        panel.content.modpack_contents(panel.server).await.expect_err("no pack now").code(),
        "modpack_not_linked"
    );
    let _ = std::fs::remove_file(&archive);
}

#[tokio::test]
async fn a_modpack_will_not_go_onto_a_running_server() {
    let panel = Panel::new("fabric", "1.21.1").await;
    let refusal = panel
        .content
        .install_modpack(
            panel.server,
            PackSource::Modrinth { project_id: "PACK".to_owned(), version_id: None },
            false,
            None,
            true,
        )
        .await
        .expect_err("it is running");
    assert_eq!(refusal.code(), "server_running");
}

#[tokio::test]
async fn the_delete_dialog_learns_what_would_be_left_needing_the_file() {
    let panel = Panel::new("fabric", "1.21.1").await;
    let api = panel.publish("API", "api-1", "api.jar", b"api");
    let mut consumer = panel.publish("MOD", "mod-1", "mod.jar", b"mod");
    consumer.dependencies = vec![super::modrinth::MrDependency {
        project_id: Some("API".to_owned()),
        version_id: None,
        dependency_type: "required".to_owned(),
    }];
    panel.upstream.set_versions("API", vec![api]);
    panel.upstream.set_versions("MOD", vec![consumer]);

    let install = panel
        .content
        .install(
            panel.server,
            &ContentInstallRequest {
                items: vec![ContentInstallTarget {
                    project_id: "MOD".to_owned(),
                    version_id: None,
                }],
                resolve_dependencies: true,
            },
            None,
        )
        .await
        .expect("an install");
    panel.settled(install.operation.id).await;

    let listed = panel.listed().await;
    let api_row = listed
        .items
        .iter()
        .find(|item| item.file_name == "api.jar")
        .expect("the dependency landed");
    let mod_row =
        listed.items.iter().find(|item| item.file_name == "mod.jar").expect("the mod landed");

    let warning = panel.content.dependents(panel.server, &[api_row.id]).await.expect("an answer");
    assert_eq!(warning.dependents.len(), 1);
    assert_eq!(warning.dependents[0].id, mod_row.id);
    assert_eq!(warning.dependents[0].depends_on, vec![api_row.id]);

    let quiet = panel.content.dependents(panel.server, &[mod_row.id]).await.expect("an answer");
    assert!(quiet.dependents.is_empty(), "nothing needs the mod itself");
}

#[tokio::test]
async fn the_preview_says_what_survives_a_game_version_change_and_what_does_not() {
    let panel = Panel::new("fabric", "1.20.1").await;
    let mut old_only = panel.publish("STUCK", "s1", "stuck.jar", b"stuck");
    old_only.game_versions = vec!["1.20.1".to_owned()];
    let mut moves_on = panel.publish("MOVER", "m1", "mover-1.jar", b"one");
    moves_on.game_versions = vec!["1.20.1".to_owned()];
    let mut moved = panel.publish("MOVER", "m2", "mover-2.jar", b"two");
    moved.game_versions = vec!["1.21.1".to_owned()];
    panel.upstream.set_versions("STUCK", vec![old_only]);
    panel.upstream.set_versions("MOVER", vec![moves_on, moved]);

    for project in ["STUCK", "MOVER"] {
        let install = panel
            .content
            .install(
                panel.server,
                &ContentInstallRequest {
                    items: vec![ContentInstallTarget {
                        project_id: project.to_owned(),
                        version_id: None,
                    }],
                    resolve_dependencies: false,
                },
                None,
            )
            .await
            .expect("an install");
        panel.settled(install.operation.id).await;
    }
    panel.write("mods/hand-made.jar", b"nobody knows");
    panel.listed().await;

    let preview = panel
        .content
        .preview(
            panel.server,
            &PreviewQuery {
                game_version: "1.21.1".to_owned(),
                loader: None,
                loader_version: None,
            },
        )
        .await
        .expect("a preview");

    assert_eq!(preview.new_game_version, "1.21.1");
    assert!(preview.has_unknown_content, "the hand made jar cannot be judged");
    assert!(preview
        .changes
        .iter()
        .any(|change| change.kind == GameVersionChangeDiffType::GameVersionUpdated));

    let stuck = preview
        .changes
        .iter()
        .find(|change| change.file_name.as_deref() == Some("stuck.jar"))
        .expect("the stuck mod");
    assert_eq!(stuck.kind, GameVersionChangeDiffType::Removed);

    let mover = preview
        .changes
        .iter()
        .find(|change| change.file_name.as_deref() == Some("mover-1.jar"))
        .expect("the mod that moves on");
    assert_eq!(mover.kind, GameVersionChangeDiffType::Updated);
    assert_eq!(mover.new_version.as_ref().expect("a target").id, "m2");
}

#[tokio::test]
async fn a_game_version_that_is_not_a_minecraft_version_is_refused_by_both_endpoints() {
    let panel = Panel::new("fabric", "1.21.1").await;

    for wrong in ["1.99.9", ""] {
        let refusal = panel
            .content
            .preview(
                panel.server,
                &PreviewQuery {
                    game_version: wrong.to_owned(),
                    loader: None,
                    loader_version: None,
                },
            )
            .await
            .expect_err("no such version");
        assert_eq!(refusal.code(), "invalid_request", "{wrong}");

        let refusal = panel
            .content
            .change_game_version(
                panel.server,
                GameVersionChangeRequest {
                    game_version: wrong.to_owned(),
                    loader: None,
                    loader_version: None,
                    incompatible_content: IncompatiblePolicy::Disable,
                },
                None,
                false,
            )
            .await
            .expect_err("no such version");
        assert_eq!(refusal.code(), "invalid_request", "{wrong}");
    }

    panel
        .content
        .preview(
            panel.server,
            &PreviewQuery {
                game_version: "1.19.2".to_owned(),
                loader: None,
                loader_version: None,
            },
        )
        .await
        .expect("a version Modrinth knows goes through");
}

#[tokio::test]
async fn changing_the_game_version_updates_what_it_can_and_switches_off_the_rest() {
    let panel = Panel::new("fabric", "1.20.1").await;
    let mut stuck = panel.publish("STUCK", "s1", "stuck.jar", b"stuck");
    stuck.game_versions = vec!["1.20.1".to_owned()];
    let mut old = panel.publish("MOVER", "m1", "mover-1.jar", b"one");
    old.game_versions = vec!["1.20.1".to_owned()];
    let mut new = panel.publish("MOVER", "m2", "mover-2.jar", b"two");
    new.game_versions = vec!["1.21.1".to_owned()];
    panel.upstream.set_versions("STUCK", vec![stuck]);
    panel.upstream.set_versions("MOVER", vec![old, new]);

    for project in ["STUCK", "MOVER"] {
        let install = panel
            .content
            .install(
                panel.server,
                &ContentInstallRequest {
                    items: vec![ContentInstallTarget {
                        project_id: project.to_owned(),
                        version_id: None,
                    }],
                    resolve_dependencies: false,
                },
                None,
            )
            .await
            .expect("an install");
        panel.settled(install.operation.id).await;
    }

    let operation = panel
        .content
        .change_game_version(
            panel.server,
            GameVersionChangeRequest {
                game_version: "1.21.1".to_owned(),
                loader: None,
                loader_version: None,
                incompatible_content: IncompatiblePolicy::UpdateThenDisable,
            },
            Some(panel.owner),
            false,
        )
        .await
        .expect("an accepted change");

    let run = panel.settled(operation.id).await;
    assert_eq!(run.state, OperationState::Done, "{:?}", run.error);
    assert!(panel.exists("mods/mover-2.jar"), "what could move on, moved on");
    assert!(panel.exists("mods/stuck.jar.disabled"), "what could not was switched off");

    let game_version: Option<String> =
        sqlx::query_scalar("SELECT game_version FROM servers WHERE id = ?")
            .bind(panel.server)
            .fetch_one(&panel.pool)
            .await
            .expect("the server");
    assert_eq!(game_version.as_deref(), Some("1.21.1"));
}

#[tokio::test]
async fn keeping_incompatible_content_really_keeps_it() {
    let panel = Panel::new("fabric", "1.20.1").await;
    let mut stuck = panel.publish("STUCK", "s1", "stuck.jar", b"stuck");
    stuck.game_versions = vec!["1.20.1".to_owned()];
    panel.upstream.set_versions("STUCK", vec![stuck]);

    let install = panel
        .content
        .install(
            panel.server,
            &ContentInstallRequest {
                items: vec![ContentInstallTarget {
                    project_id: "STUCK".to_owned(),
                    version_id: None,
                }],
                resolve_dependencies: false,
            },
            None,
        )
        .await
        .expect("an install");
    panel.settled(install.operation.id).await;

    let operation = panel
        .content
        .change_game_version(
            panel.server,
            GameVersionChangeRequest {
                game_version: "1.21.1".to_owned(),
                loader: None,
                loader_version: None,
                incompatible_content: IncompatiblePolicy::Keep,
            },
            None,
            false,
        )
        .await
        .expect("an accepted change");
    panel.settled(operation.id).await;

    assert!(panel.exists("mods/stuck.jar"));
    assert!(!panel.exists("mods/stuck.jar.disabled"));
}

#[tokio::test]
async fn an_upload_lands_in_the_content_directory_and_the_tree_goes_back() {
    let panel = Panel::new("paper", "1.21.1").await;
    let staged = panel.root.join(crate::ops::WORK_DIR).join("upload").join("part-1");
    std::fs::create_dir_all(staged.parent().expect("a parent")).expect("a directory");
    std::fs::write(&staged, b"a plugin jar").expect("a staged upload");

    let answer = panel
        .content
        .adopt_uploads(panel.server, vec![("nice.jar".to_owned(), staged, 12)])
        .await
        .expect("an upload");

    assert!(answer.results[0].ok);
    assert!(panel.exists("plugins/nice.jar"));
    assert_eq!(panel.chowned(), 1);
}

#[tokio::test]
async fn only_jars_and_zips_may_be_uploaded_here() {
    let panel = Panel::new("fabric", "1.21.1").await;
    let staged = panel.root.join(crate::ops::WORK_DIR).join("upload").join("part-1");
    std::fs::create_dir_all(staged.parent().expect("a parent")).expect("a directory");
    std::fs::write(&staged, b"pack").expect("a staged upload");

    let answer = panel
        .content
        .adopt_uploads(panel.server, vec![("pack.mrpack".to_owned(), staged, 4)])
        .await
        .expect("an upload");
    assert!(!answer.results[0].ok);
    assert_eq!(answer.results[0].error.as_deref(), Some("unsupported_file_type"));
    assert_eq!(panel.chowned(), 0, "nothing was written, so nothing is handed back");
}

#[tokio::test]
async fn a_server_with_a_run_on_it_refuses_a_second_write() {
    let panel = Panel::new("fabric", "1.21.1").await;
    panel.write("mods/foo.jar", b"jar");
    let id = panel.listed().await.items[0].id;

    panel
        .operations
        .create(crate::ops::NewOperation::new(
            panel.server,
            crate::model::OperationKind::BackupCreate,
            None,
        ))
        .await
        .expect("a backup run");

    let refusal = panel
        .content
        .install(
            panel.server,
            &ContentInstallRequest {
                items: vec![ContentInstallTarget {
                    project_id: "MOD".to_owned(),
                    version_id: None,
                }],
                resolve_dependencies: false,
            },
            None,
        )
        .await
        .expect_err("something else is going on");
    assert_eq!(refusal.code(), "server_busy");
    assert_eq!(refusal.status(), axum::http::StatusCode::CONFLICT);

    assert!(panel.content.set_enabled(panel.server, &[id], false).await.is_ok());
}

#[tokio::test]
async fn an_administrator_can_switch_the_outside_world_off() {
    let panel = Panel::new("fabric", "1.21.1").await;
    assert!(panel.content.modrinth().allowed().await);

    sqlx::query("UPDATE panel_settings SET external_services_enabled = 0 WHERE id = 1")
        .execute(&panel.pool)
        .await
        .expect("the switch");
    assert!(!panel.content.modrinth().allowed().await);
}

#[tokio::test]
async fn the_upload_ceiling_is_the_one_the_file_manager_uses() {
    let panel = Panel::new("fabric", "1.21.1").await;
    assert_eq!(
        panel.content.max_upload_bytes().await.expect("a ceiling"),
        4 * 1024 * 1024 * 1024,
        "8.8: one panel, one upload limit"
    );
}

#[tokio::test]
async fn a_stale_check_starts_itself_and_a_fresh_one_does_not() {
    let panel = Panel::new("fabric", "1.21.1").await;
    store::checked_at(&panel.pool, panel.server, Timestamp::now()).await.expect("a check time");

    let before = panel.upstream.calls();
    panel.listed().await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(panel.upstream.calls(), before, "8.16: six hours old is not stale");
}

#[tokio::test]
#[ignore = "installs a real mod from api.modrinth.com"]
async fn live_a_real_mod_installs_from_modrinth() {
    let panel = Panel::new("fabric", "1.20.1").await;
    let real = Arc::new(super::Modrinth::new(panel.pool.clone()).expect("a client"));
    let content = Content::with_modrinth(
        panel.pool.clone(),
        panel.root.parent().expect("servers").parent().expect("user").parent().expect("users")
            .parent().expect("the data directory"),
        Helper::new(panel.helper.socket()),
        Arc::clone(&panel.operations),
        real,
        Disks::none(),
    );

    let answer = content
        .install(
            panel.server,
            &ContentInstallRequest {
                items: vec![ContentInstallTarget {
                    project_id: "gvQqBUqZ".to_owned(),
                    version_id: None,
                }],
                resolve_dependencies: true,
            },
            None,
        )
        .await
        .expect("an accepted install");
    assert_eq!(answer.planned.len(), 1);

    let run = panel.settled(answer.operation.id).await;
    assert_eq!(run.state, OperationState::Done, "{:?}", run.error);

    let laid_out: Vec<_> = std::fs::read_dir(panel.root.join("mods"))
        .expect("the mods directory")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(laid_out.len(), 1, "{laid_out:?}");
    assert!(laid_out[0].starts_with("lithium"), "{laid_out:?}");
    assert!(panel.chowned() >= 1);
}

#[tokio::test]
async fn updating_a_pack_drops_what_it_no_longer_brings_and_leaves_what_the_user_added() {
    let panel = Panel::new("fabric", "1.21.1").await;
    let first = b"the first mod";
    let second = b"the second mod";
    let first_url = panel.upstream.add_file("first.jar", first.to_vec());
    let second_url = panel.upstream.add_file("second.jar", second.to_vec());

    let both = a_pack(&[(
        "modrinth.index.json",
        &index(serde_json::json!([
            {
                "path": "mods/first.jar",
                "downloads": [first_url.clone()],
                "hashes": { "sha512": hex::encode(Sha512::digest(first)) }
            },
            {
                "path": "mods/second.jar",
                "downloads": [second_url],
                "hashes": { "sha512": hex::encode(Sha512::digest(second)) }
            }
        ])),
    )]);
    let operation = panel
        .content
        .install_modpack(
            panel.server,
            PackSource::Upload { archive: both.clone(), file_name: "v1.mrpack".to_owned() },
            false,
            None,
            false,
        )
        .await
        .expect("an install");
    panel.settled(operation.id).await;
    assert!(panel.exists("mods/first.jar") && panel.exists("mods/second.jar"));

    panel.write("mods/mine.jar", b"added by hand");
    panel.listed().await;

    let slimmer = a_pack(&[(
        "modrinth.index.json",
        &index(serde_json::json!([{
            "path": "mods/first.jar",
            "downloads": [first_url],
            "hashes": { "sha512": hex::encode(Sha512::digest(first)) }
        }])),
    )]);
    let operation = panel
        .content
        .install_modpack(
            panel.server,
            PackSource::Upload { archive: slimmer.clone(), file_name: "v2.mrpack".to_owned() },
            true,
            None,
            false,
        )
        .await
        .expect("an update");
    let run = panel.settled(operation.id).await;
    assert_eq!(run.state, OperationState::Done, "{:?}", run.error);

    assert!(panel.exists("mods/first.jar"));
    assert!(!panel.exists("mods/second.jar"), "8.11: what the pack dropped, we drop");
    assert!(panel.exists("mods/mine.jar"), "8.11: what the user added stays untouched");

    let listed = panel.listed().await;
    assert_eq!(listed.items.len(), 1, "only the hand added mod is in the main list");
    assert_eq!(listed.items[0].file_name, "mine.jar");

    let _ = std::fs::remove_file(&both);
    let _ = std::fs::remove_file(&slimmer);
}

#[tokio::test]
async fn a_content_directory_that_is_a_link_out_of_the_server_is_not_read() {
    let panel = Panel::new("fabric", "1.21.1").await;
    let elsewhere = panel.root.parent().expect("a parent").join("not-ours");
    std::fs::create_dir_all(&elsewhere).expect("a directory outside");
    std::fs::write(elsewhere.join("victim.jar"), b"somebody else's mod").expect("a file");
    std::os::unix::fs::symlink(&elsewhere, panel.root.join("mods")).expect("a link");

    let listed = panel.listed().await;
    let names: Vec<&str> = listed.items.iter().map(|item| item.file_name.as_str()).collect();
    assert!(names.is_empty(), "a linked mods directory must not be read: {names:?}");
}

#[tokio::test]
async fn an_upload_whose_name_climbs_out_writes_nothing_outside() {
    let panel = Panel::new("fabric", "1.21.1").await;
    let staged = panel.root.join(crate::ops::WORK_DIR).join("upload").join("part-1");
    std::fs::create_dir_all(staged.parent().expect("a parent")).expect("a directory");
    std::fs::write(&staged, b"malice").expect("a staged upload");

    let answer = panel
        .content
        .adopt_uploads(panel.server, vec![("../../../../escaped.jar".to_owned(), staged, 6)])
        .await
        .expect("an answer");
    assert!(!answer.results[0].ok, "{:?}", answer.results[0]);
    let above = panel.root.join("..").join("..").join("..").join("..").join("escaped.jar");
    assert!(!above.exists(), "an upload must not name anything outside");
}

#[tokio::test]
async fn a_pack_file_that_lands_on_a_link_leaves_what_the_link_pointed_at() {
    let panel = Panel::new("fabric", "1.21.1").await;
    let outside = panel.root.parent().expect("a parent").join("target.txt");
    std::fs::write(&outside, b"keep me").expect("a file outside");
    std::fs::create_dir_all(panel.root.join("mods")).expect("a directory");
    std::os::unix::fs::symlink(&outside, panel.root.join("mods").join("packed.jar"))
        .expect("a link the game process laid");

    let body = b"a packed mod";
    let url = panel.upstream.add_file("packed.jar", body.to_vec());
    let archive = a_pack(&[(
        "modrinth.index.json",
        &index(serde_json::json!([{
            "path": "mods/packed.jar",
            "downloads": [url],
            "hashes": { "sha512": hex::encode(Sha512::digest(body)) }
        }])),
    )]);
    let operation = panel
        .content
        .install_modpack(
            panel.server,
            PackSource::Upload { archive: archive.clone(), file_name: "p.mrpack".to_owned() },
            false,
            None,
            false,
        )
        .await
        .expect("an install");
    panel.settled(operation.id).await;

    assert_eq!(std::fs::read(&outside).expect("the target stays"), b"keep me");
    let _ = std::fs::remove_file(&archive);
    let _ = std::fs::remove_file(&outside);
}

#[tokio::test]
async fn a_run_that_gives_up_halfway_still_hands_back_what_it_laid_down() {
    let panel = Panel::new("fabric", "1.21.1").await;
    let good = panel.publish("GOOD", "g1", "good.jar", b"the good one");
    let mut bad = panel.publish("BAD", "b1", "bad.jar", b"the bad one");
    bad.files[0].hashes.sha512 = Some("00".repeat(64));
    panel.upstream.set_versions("GOOD", vec![good]);
    panel.upstream.set_versions("BAD", vec![bad]);

    let answer = panel
        .content
        .install(
            panel.server,
            &ContentInstallRequest {
                items: vec![
                    ContentInstallTarget { project_id: "GOOD".to_owned(), version_id: None },
                    ContentInstallTarget { project_id: "BAD".to_owned(), version_id: None },
                ],
                resolve_dependencies: false,
            },
            None,
        )
        .await
        .expect("an accepted install");
    let run = panel.settled(answer.operation.id).await;

    assert_eq!(run.state, OperationState::Failed);
    assert!(panel.exists("mods/good.jar"), "the first one was already down");
    assert_eq!(
        panel.chowned(),
        1,
        "a file a failed run left behind still has to belong to the account"
    );
}

#[tokio::test]
async fn an_uploaded_pack_does_not_stay_behind_in_the_server_directory() {
    let panel = Panel::new("fabric", "1.21.1").await;
    let staging = panel.root.join(crate::ops::WORK_DIR).join(format!("pack-{}", Id::new()));
    std::fs::create_dir_all(&staging).expect("a staging directory");
    let archive = staging.join("part-1");
    std::fs::copy(a_pack(&[("modrinth.index.json", &index(serde_json::json!([])))]), &archive)
        .expect("the uploaded pack");

    let operation = panel
        .content
        .install_modpack(
            panel.server,
            PackSource::Upload { archive: archive.clone(), file_name: "p.mrpack".to_owned() },
            false,
            None,
            false,
        )
        .await
        .expect("an install");
    let run = panel.settled(operation.id).await;
    assert_eq!(run.state, OperationState::Done, "{:?}", run.error);

    assert!(gone(&archive).await, "the uploaded pack goes when the run is over");
    assert!(gone(&staging).await, "and so does the directory it sat in");
}

async fn gone(path: &std::path::Path) -> bool {
    for _ in 0..200 {
        if !path.exists() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    false
}

#[tokio::test]
async fn a_pack_that_fails_takes_its_upload_with_it_all_the_same() {
    let panel = Panel::new("fabric", "1.21.1").await;
    let staging = panel.root.join(crate::ops::WORK_DIR).join(format!("pack-{}", Id::new()));
    std::fs::create_dir_all(&staging).expect("a staging directory");
    let archive = staging.join("part-1");
    std::fs::write(&archive, b"this is not a zip").expect("the uploaded pack");

    let operation = panel
        .content
        .install_modpack(
            panel.server,
            PackSource::Upload { archive: archive.clone(), file_name: "p.mrpack".to_owned() },
            false,
            None,
            false,
        )
        .await
        .expect("an install");
    let run = panel.settled(operation.id).await;
    assert_eq!(run.state, OperationState::Failed);
    assert_eq!(run.error.expect("an error").code, "unsupported_archive");
    assert!(gone(&staging).await);
}

#[tokio::test]
async fn a_second_read_does_not_start_a_second_background_check() {
    let panel = Panel::new("fabric", "1.21.1").await;
    panel.write("mods/one.jar", b"jar");
    let scanned = super::scan::reconcile(&panel.pool, panel.server, &panel.root)
        .await
        .expect("a scan");
    let mut row = scanned.into_iter().next().expect("the row");
    row.project_id = Some("P1".to_owned());
    row.version_id = Some("v-old".to_owned());
    store::upsert(&panel.pool, &row).await.expect("an origin");

    let before = panel.upstream.calls();
    for _ in 0..8 {
        panel.content.list(panel.access(), true).await.expect("a list");
    }
    for _ in 0..80 {
        let checked: Option<Timestamp> =
            sqlx::query_scalar("SELECT updates_checked_at FROM servers WHERE id = ?")
                .bind(panel.server)
                .fetch_one(&panel.pool)
                .await
                .expect("the server row");
        if checked.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    tokio::time::sleep(Duration::from_millis(100)).await;

    let spent = panel.upstream.calls() - before;
    assert!(spent <= 2, "eight reads, one project, {spent} calls to Modrinth");
}

#[tokio::test]
async fn the_preview_names_the_installed_version_by_its_number() {
    let panel = Panel::new("fabric", "1.20.1").await;
    let mut only_old = panel.publish("STUCK", "AbCd1234", "stuck.jar", b"stuck");
    only_old.version_number = "3.1.0".to_owned();
    only_old.game_versions = vec!["1.20.1".to_owned()];
    panel.upstream.set_versions("STUCK", vec![only_old]);

    let install = panel
        .content
        .install(panel.server, &wants("STUCK", None), None)
        .await
        .expect("an install");
    panel.settled(install.operation.id).await;

    let preview = panel
        .content
        .preview(
            panel.server,
            &PreviewQuery {
                game_version: "1.21.1".to_owned(),
                loader: None,
                loader_version: None,
            },
        )
        .await
        .expect("a preview");

    let stuck = preview
        .changes
        .iter()
        .find(|change| change.file_name.as_deref() == Some("stuck.jar"))
        .expect("the stuck mod");
    let current = stuck.current_version.as_ref().expect("what is installed now");
    assert_eq!(current.id, "AbCd1234");
    assert_eq!(current.version_number, "3.1.0", "8.13: the number, not the id");
}

#[tokio::test]
async fn uploading_the_same_name_twice_replaces_the_file_and_keeps_the_row() {
    let panel = Panel::new("fabric", "1.21.1").await;
    let mut ids = Vec::new();
    for (round, body) in [(0, &b"the first jar"[..]), (1, &b"the second jar"[..])] {
        let staged = panel.root.join(crate::ops::WORK_DIR).join(format!("u{round}")).join("part");
        std::fs::create_dir_all(staged.parent().expect("a parent")).expect("a directory");
        std::fs::write(&staged, body).expect("a staged upload");
        let answer = panel
            .content
            .adopt_uploads(panel.server, vec![("same.jar".to_owned(), staged, body.len() as u64)])
            .await
            .expect("an upload");
        assert!(answer.results[0].ok, "{:?}", answer.results[0]);
        ids.push(answer.results[0].id.expect("a row"));
    }

    assert_eq!(ids[0], ids[1], "8.1: the same file keeps its id");
    assert_eq!(
        std::fs::read(panel.root.join("mods").join("same.jar")).expect("the jar"),
        b"the second jar"
    );
    assert_eq!(store::list(&panel.pool, panel.server).await.expect("the rows").len(), 1);
    assert_eq!(panel.chowned(), 2, "both uploads hand the tree back");
}

#[tokio::test]
async fn installing_over_a_file_that_was_already_there_keeps_one_row() {
    let panel = Panel::new("fabric", "1.21.1").await;
    panel.write("mods/thing.jar", b"dropped in by hand");
    let known = panel.listed().await.items[0].id;

    let version = panel.publish("MOD", "v1", "thing.jar", b"the real thing");
    panel.upstream.set_versions("MOD", vec![version]);
    let answer =
        panel.content.install(panel.server, &wants("MOD", None), None).await.expect("an install");
    let run = panel.settled(answer.operation.id).await;
    assert_eq!(run.state, OperationState::Done, "{:?}", run.error);

    let listed = panel.listed().await;
    assert_eq!(listed.items.len(), 1);
    assert_eq!(listed.items[0].id, known, "the row of the file that was already there");
    assert_eq!(
        std::fs::read(panel.root.join("mods").join("thing.jar")).expect("the jar"),
        b"the real thing"
    );
    assert_eq!(
        panel.row(known).await.expect("the row").project_id.as_deref(),
        Some("MOD"),
        "and it now knows where it came from"
    );
}

#[tokio::test]
async fn uploading_over_a_switched_off_file_takes_the_switched_off_one_away() {
    let panel = Panel::new("fabric", "1.21.1").await;
    panel.write("mods/thing.jar.disabled", b"switched off");
    let known = panel.listed().await.items[0].id;

    let staged = panel.root.join(crate::ops::WORK_DIR).join("u").join("part");
    std::fs::create_dir_all(staged.parent().expect("a parent")).expect("a directory");
    std::fs::write(&staged, b"a fresh copy").expect("a staged upload");
    let answer = panel
        .content
        .adopt_uploads(panel.server, vec![("thing.jar".to_owned(), staged, 12)])
        .await
        .expect("an upload");

    assert_eq!(answer.results[0].id, Some(known));
    assert!(panel.exists("mods/thing.jar"));
    assert!(!panel.exists("mods/thing.jar.disabled"));
    let listed = panel.listed().await;
    assert_eq!(listed.items.len(), 1);
    assert!(listed.items[0].enabled);
}
