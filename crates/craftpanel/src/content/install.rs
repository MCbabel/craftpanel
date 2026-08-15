use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::model::{ContentProjectType, Id, LoaderFamily, LoaderId, UpdateChannel};

use super::channels;
use super::compat::{self, Target};
use super::modrinth::{MrFile, MrVersion, Modrinth, Upstream};
use super::paths;
use super::store::ItemRow;
use super::types::{
    ContentInstallTarget, ContentPlanEntry, ContentSkipReason, ContentSkippedEntry, PlanReason,
};

const FABRIC_API: &str = "P7dR8mSH";

#[derive(Debug, Clone)]
pub struct Wanted {
    pub project_id: String,
    pub version: MrVersion,
    pub file: MrFile,
    pub reason: PlanReason,
    pub replaces: Option<Id>,
}

impl Wanted {
    pub fn entry(&self) -> ContentPlanEntry {
        ContentPlanEntry {
            project_id: self.project_id.clone(),
            version_id: self.version.id.clone(),
            file_name: self.file.filename.clone(),
            reason: self.reason,
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct Plan {
    pub wanted: Vec<Wanted>,
    pub skipped: Vec<ContentSkippedEntry>,
}

impl Plan {
    pub fn entries(&self) -> Vec<ContentPlanEntry> {
        self.wanted.iter().map(Wanted::entry).collect()
    }
}

pub fn directory_of(loader: LoaderId) -> &'static str {
    match loader.family() {
        LoaderFamily::Vanilla => "world/datapacks",
        LoaderFamily::Bukkit | LoaderFamily::Proxy => "plugins",
        LoaderFamily::Modloader => "mods",
    }
}

pub async fn resolve(
    modrinth: &Modrinth,
    target: &Target,
    channel: UpdateChannel,
    installed: &[ItemRow],
    requests: &[ContentInstallTarget],
    follow_dependencies: bool,
) -> Result<Plan, Upstream> {
    let already: BTreeSet<String> =
        installed.iter().filter_map(|row| row.project_id.clone()).collect();

    let mut plan = Plan::default();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut pinned: BTreeMap<String, String> = BTreeMap::new();
    let mut queue: Vec<(String, Option<String>, PlanReason)> = requests
        .iter()
        .map(|item| (item.project_id.clone(), item.version_id.clone(), PlanReason::Requested))
        .collect();

    while let Some((project_id, version_id, reason)) = pop(&mut queue) {
        if !seen.insert(project_id.clone()) {
            let clashes = version_id
                .as_ref()
                .zip(pinned.get(&project_id))
                .is_some_and(|(wanted, planned)| wanted != planned);
            let reason = if clashes {
                ContentSkipReason::ConflictingDependency
            } else {
                ContentSkipReason::DuplicateProject
            };
            plan.skipped.push(skip(&project_id, version_id, reason));
            continue;
        }
        if already.contains(&project_id) {
            plan.skipped.push(skip(&project_id, version_id, ContentSkipReason::AlreadyInstalled));
            continue;
        }
        if project_id == FABRIC_API && target.loader == LoaderId::Quilt {
            plan.skipped.push(skip(&project_id, version_id, ContentSkipReason::QuiltFabricApi));
            continue;
        }

        let chosen = match &version_id {
            Some(wanted) => match modrinth.version(wanted).await {
                Ok(version) => Some(version),
                Err(Upstream::NotFound(_)) => None,
                Err(err) => return Err(err),
            },
            None => newest_fitting(modrinth, &project_id, target, channel).await?,
        };

        let Some(version) = chosen else {
            let reason = match version_id {
                Some(_) => ContentSkipReason::MissingVersion,
                None => ContentSkipReason::NoCompatibleVersion,
            };
            plan.skipped.push(skip(&project_id, None, reason));
            continue;
        };
        let Some(file) = version.primary_file().cloned() else {
            plan.skipped.push(skip(
                &project_id,
                Some(version.id.clone()),
                ContentSkipReason::MissingVersion,
            ));
            continue;
        };

        if follow_dependencies {
            for dependency in version.requires() {
                queue.push((dependency.to_owned(), None, PlanReason::Dependency));
            }
        }

        pinned.insert(project_id.clone(), version.id.clone());
        plan.wanted.push(Wanted { project_id, version, file, reason, replaces: None });
    }

    Ok(plan)
}

fn pop(queue: &mut Vec<(String, Option<String>, PlanReason)>) -> Option<(String, Option<String>, PlanReason)> {
    if queue.is_empty() {
        None
    } else {
        Some(queue.remove(0))
    }
}

fn skip(
    project_id: &str,
    version_id: Option<String>,
    reason: ContentSkipReason,
) -> ContentSkippedEntry {
    ContentSkippedEntry { project_id: project_id.to_owned(), version_id, reason }
}

pub async fn newest_fitting(
    modrinth: &Modrinth,
    project_id: &str,
    target: &Target,
    channel: UpdateChannel,
) -> Result<Option<MrVersion>, Upstream> {
    let project_type = match modrinth.cached_project(project_id).await? {
        Some(project) => project.project_type.as_deref().and_then(kind_of),
        None => None,
    };
    let target = Target { project_type, ..target.clone() };

    let versions = match modrinth.versions(project_id).await {
        Ok(versions) => versions,
        Err(Upstream::NotFound(_)) => return Ok(None),
        Err(err) => return Err(err),
    };

    let mut fitting: Vec<MrVersion> = versions
        .into_iter()
        .filter(|version| compat::matches(version, &target))
        .filter(|version| channels::allows(&version.version_type, channel, None))
        .collect();
    fitting.sort_by(|left, right| right.published().cmp(&left.published()));
    Ok(fitting.into_iter().next())
}

pub fn kind_of(project_type: &str) -> Option<ContentProjectType> {
    match project_type {
        "mod" => Some(ContentProjectType::Mod),
        "plugin" => Some(ContentProjectType::Plugin),
        "datapack" => Some(ContentProjectType::Datapack),
        "resourcepack" => Some(ContentProjectType::Resourcepack),
        "shader" | "shaderpack" => Some(ContentProjectType::Shader),
        _ => None,
    }
}

pub async fn update_for(
    modrinth: &Modrinth,
    row: &ItemRow,
    target: &Target,
    channel: UpdateChannel,
) -> Result<Option<MrVersion>, Upstream> {
    let (Some(project_id), Some(version_id)) = (&row.project_id, &row.version_id) else {
        return Ok(None);
    };

    let versions = match modrinth.versions(project_id).await {
        Ok(versions) => versions,
        Err(Upstream::NotFound(_)) => return Ok(None),
        Err(err) => return Err(err),
    };
    let installed = versions.iter().find(|version| &version.id == version_id);
    let installed_type = installed.map(|version| version.version_type.clone());
    let published = installed.and_then(MrVersion::published);

    let fitting: Vec<MrVersion> = versions
        .into_iter()
        .filter(|version| compat::matches(version, target))
        .collect();

    Ok(channels::newest_eligible(
        &fitting,
        version_id,
        published,
        channel,
        installed_type.as_deref(),
    )
    .cloned())
}

pub async fn fetch(modrinth: &Modrinth, wanted: &Wanted, work_dir: &Path) -> Result<PathBuf, Upstream> {
    let name = super::multipart::safe_file_name(&wanted.file.filename)
        .unwrap_or_else(|_| format!("{}.jar", wanted.version.id));
    let staged = work_dir.join(format!("{}-{name}", wanted.version.id));
    modrinth.download(&wanted.file.url, &staged, &wanted.file.hashes).await?;
    Ok(staged)
}

pub fn place(staged: &Path, root: &Path, relative: &str) -> std::io::Result<()> {
    let target = paths::resolve_leaf(root, relative)
        .map_err(|fault| std::io::Error::other(fault.message()))?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    paths::clear_destination(&target)?;
    match std::fs::rename(staged, &target) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::copy(staged, &target)?;
            std::fs::remove_file(staged)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::harness::{client, fake_modrinth, schema};
    use crate::content::modrinth::{a_version, MrDependency};

    fn requested(project: &str) -> ContentInstallTarget {
        ContentInstallTarget { project_id: project.to_owned(), version_id: None }
    }

    fn target(loader: LoaderId) -> Target {
        Target::new("1.21.1", loader)
    }

    #[test]
    fn each_loader_family_installs_into_its_own_directory() {
        assert_eq!(directory_of(LoaderId::Fabric), "mods");
        assert_eq!(directory_of(LoaderId::NeoForge), "mods");
        assert_eq!(directory_of(LoaderId::Paper), "plugins");
        assert_eq!(directory_of(LoaderId::Velocity), "plugins");
        assert_eq!(directory_of(LoaderId::Vanilla), "world/datapacks");
    }

    #[tokio::test]
    async fn a_required_dependency_is_planned_beside_what_was_asked_for() {
        let pool = schema().await;
        let upstream = fake_modrinth().await;
        let mut needs_api = a_version("needs-api", "MOD", "release", "2026-06-01T00:00:00Z");
        needs_api.dependencies = vec![MrDependency {
            project_id: Some(FABRIC_API.to_owned()),
            version_id: None,
            dependency_type: "required".to_owned(),
        }];
        upstream.set_versions("MOD", vec![needs_api]);
        upstream.set_versions(
            FABRIC_API,
            vec![a_version("api", FABRIC_API, "release", "2026-06-01T00:00:00Z")],
        );

        let modrinth = client(&pool, &upstream);
        let plan = resolve(
            &modrinth,
            &target(LoaderId::Fabric),
            UpdateChannel::Release,
            &[],
            &[requested("MOD")],
            true,
        )
        .await
        .expect("a plan");

        let entries = plan.entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].reason, PlanReason::Requested);
        assert_eq!(entries[1].project_id, FABRIC_API);
        assert_eq!(entries[1].reason, PlanReason::Dependency);
    }

    #[tokio::test]
    async fn a_dependency_that_is_already_on_the_disk_is_skipped_and_said_so() {
        let pool = schema().await;
        let upstream = fake_modrinth().await;
        let mut needs_api = a_version("needs-api", "MOD", "release", "2026-06-01T00:00:00Z");
        needs_api.dependencies = vec![MrDependency {
            project_id: Some(FABRIC_API.to_owned()),
            version_id: None,
            dependency_type: "required".to_owned(),
        }];
        upstream.set_versions("MOD", vec![needs_api]);

        let mut installed = ItemRow::fresh(Id::new(), "mods/api.jar", ContentProjectType::Mod);
        installed.project_id = Some(FABRIC_API.to_owned());

        let modrinth = client(&pool, &upstream);
        let plan = resolve(
            &modrinth,
            &target(LoaderId::Fabric),
            UpdateChannel::Release,
            std::slice::from_ref(&installed),
            &[requested("MOD")],
            true,
        )
        .await
        .expect("a plan");

        assert_eq!(plan.wanted.len(), 1);
        assert_eq!(plan.skipped.len(), 1);
        assert_eq!(plan.skipped[0].reason, ContentSkipReason::AlreadyInstalled);
        assert_eq!(plan.skipped[0].project_id, FABRIC_API);
    }

    #[tokio::test]
    async fn quilt_is_told_it_already_has_fabric_api() {
        let pool = schema().await;
        let upstream = fake_modrinth().await;
        upstream.set_versions(
            FABRIC_API,
            vec![a_version("api", FABRIC_API, "release", "2026-06-01T00:00:00Z")],
        );

        let modrinth = client(&pool, &upstream);
        let plan = resolve(
            &modrinth,
            &target(LoaderId::Quilt),
            UpdateChannel::Release,
            &[],
            &[requested(FABRIC_API)],
            true,
        )
        .await
        .expect("a plan");

        assert!(plan.wanted.is_empty());
        assert_eq!(plan.skipped[0].reason, ContentSkipReason::QuiltFabricApi);
    }

    #[tokio::test]
    async fn a_mod_with_nothing_for_this_loader_is_reported_not_installed() {
        let pool = schema().await;
        let upstream = fake_modrinth().await;
        let mut fabric_only = a_version("only-fabric", "MOD", "release", "2026-06-01T00:00:00Z");
        fabric_only.loaders = vec!["fabric".to_owned()];
        upstream.set_versions("MOD", vec![fabric_only]);

        let modrinth = client(&pool, &upstream);
        let plan = resolve(
            &modrinth,
            &target(LoaderId::Paper),
            UpdateChannel::Release,
            &[],
            &[requested("MOD")],
            true,
        )
        .await
        .expect("a plan");

        assert!(plan.wanted.is_empty());
        assert_eq!(plan.skipped[0].reason, ContentSkipReason::NoCompatibleVersion);
    }

    #[tokio::test]
    async fn a_version_chosen_by_hand_is_installed_without_a_compatibility_check() {
        let pool = schema().await;
        let upstream = fake_modrinth().await;
        let mut wrong_game = a_version("hand-picked", "MOD", "release", "2026-06-01T00:00:00Z");
        wrong_game.game_versions = vec!["1.7.10".to_owned()];
        upstream.set_versions("MOD", vec![wrong_game]);

        let modrinth = client(&pool, &upstream);
        let plan = resolve(
            &modrinth,
            &target(LoaderId::Fabric),
            UpdateChannel::Release,
            &[],
            &[ContentInstallTarget {
                project_id: "MOD".to_owned(),
                version_id: Some("hand-picked".to_owned()),
            }],
            true,
        )
        .await
        .expect("a plan");

        assert_eq!(plan.wanted.len(), 1, "8.7: what the user picks, we install");
        assert!(plan.skipped.is_empty());
    }

    #[tokio::test]
    async fn asking_for_the_same_project_twice_installs_it_once() {
        let pool = schema().await;
        let upstream = fake_modrinth().await;
        upstream
            .set_versions("MOD", vec![a_version("v", "MOD", "release", "2026-06-01T00:00:00Z")]);

        let modrinth = client(&pool, &upstream);
        let plan = resolve(
            &modrinth,
            &target(LoaderId::Fabric),
            UpdateChannel::Release,
            &[],
            &[requested("MOD"), requested("MOD")],
            true,
        )
        .await
        .expect("a plan");

        assert_eq!(plan.wanted.len(), 1);
        assert_eq!(plan.skipped[0].reason, ContentSkipReason::DuplicateProject);
    }

    #[tokio::test]
    async fn dependencies_are_left_alone_when_the_caller_says_so() {
        let pool = schema().await;
        let upstream = fake_modrinth().await;
        let mut needs_api = a_version("needs-api", "MOD", "release", "2026-06-01T00:00:00Z");
        needs_api.dependencies = vec![MrDependency {
            project_id: Some(FABRIC_API.to_owned()),
            version_id: None,
            dependency_type: "required".to_owned(),
        }];
        upstream.set_versions("MOD", vec![needs_api]);

        let modrinth = client(&pool, &upstream);
        let plan = resolve(
            &modrinth,
            &target(LoaderId::Fabric),
            UpdateChannel::Release,
            &[],
            &[requested("MOD")],
            false,
        )
        .await
        .expect("a plan");
        assert_eq!(plan.wanted.len(), 1);
        assert!(plan.skipped.is_empty());
    }

    #[tokio::test]
    async fn an_optional_dependency_is_not_ours_to_install() {
        let pool = schema().await;
        let upstream = fake_modrinth().await;
        let mut with_extras = a_version("extras", "MOD", "release", "2026-06-01T00:00:00Z");
        with_extras.dependencies = vec![
            MrDependency {
                project_id: Some("OPT".to_owned()),
                version_id: None,
                dependency_type: "optional".to_owned(),
            },
            MrDependency {
                project_id: Some("EMB".to_owned()),
                version_id: None,
                dependency_type: "embedded".to_owned(),
            },
        ];
        upstream.set_versions("MOD", vec![with_extras]);

        let modrinth = client(&pool, &upstream);
        let plan = resolve(
            &modrinth,
            &target(LoaderId::Fabric),
            UpdateChannel::Release,
            &[],
            &[requested("MOD")],
            true,
        )
        .await
        .expect("a plan");
        assert_eq!(plan.wanted.len(), 1, "only `required` is followed (8.7)");
    }
}
