use serde::{Deserialize, Serialize};

use crate::model::{
    ApiContentItem, ContentModpack, ContentProjectType, LoaderId, Operation, Timestamp,
    UpdateChannel,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentPermissions {
    pub can_read: bool,
    pub can_write: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ContentListResponse {
    pub content_type: ContentProjectType,
    pub loader: LoaderId,
    pub loader_version: Option<String>,
    pub game_version: String,
    pub update_channel: UpdateChannel,
    pub updates_checked_at: Option<Timestamp>,
    pub permissions: ContentPermissions,
    pub modpack: Option<ContentModpack>,
    pub items: Vec<ApiContentItem>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModpackContentsResponse {
    pub items: Vec<ApiContentItem>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ContentIdsRequest {
    pub ids: Vec<crate::model::Id>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ContentMutationResult {
    pub id: crate::model::Id,
    pub ok: bool,
    pub file_name: Option<String>,
    pub file_path: Option<String>,
    pub enabled: Option<bool>,
    pub error: Option<String>,
    pub message: Option<String>,
}

impl ContentMutationResult {
    pub fn failed(id: crate::model::Id, error: &str, message: impl Into<String>) -> Self {
        Self {
            id,
            ok: false,
            file_name: None,
            file_path: None,
            enabled: None,
            error: Some(error.to_owned()),
            message: Some(message.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ContentMutationResponse {
    pub results: Vec<ContentMutationResult>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ContentUpdateTarget {
    pub id: crate::model::Id,
    #[serde(default)]
    pub version_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ContentUpdateRequest {
    #[serde(default)]
    pub items: Vec<ContentUpdateTarget>,
    #[serde(default)]
    pub all: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ContentUpdateResponse {
    pub operation: Operation,
    pub total: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ContentInstallTarget {
    pub project_id: String,
    #[serde(default)]
    pub version_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ContentInstallRequest {
    #[serde(default)]
    pub items: Vec<ContentInstallTarget>,
    #[serde(default = "yes")]
    pub resolve_dependencies: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentSkipReason {
    AlreadyInstalled,
    DuplicateProject,
    ConflictingDependency,
    NoCompatibleVersion,
    MissingVersion,
    QuiltFabricApi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanReason {
    Requested,
    Dependency,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ContentPlanEntry {
    pub project_id: String,
    pub version_id: String,
    pub file_name: String,
    pub reason: PlanReason,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ContentSkippedEntry {
    pub project_id: String,
    pub version_id: Option<String>,
    pub reason: ContentSkipReason,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ContentInstallResponse {
    pub operation: Operation,
    pub planned: Vec<ContentPlanEntry>,
    pub skipped: Vec<ContentSkippedEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ContentUploadResult {
    pub file_name: String,
    pub ok: bool,
    pub id: Option<crate::model::Id>,
    pub error: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ContentUploadResponse {
    pub results: Vec<ContentUploadResult>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ContentDependentEntry {
    pub id: crate::model::Id,
    pub depends_on: Vec<crate::model::Id>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ContentDependentsResponse {
    pub dependents: Vec<ContentDependentEntry>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ModpackSource {
    Modrinth {
        project_id: String,
        #[serde(default)]
        version_id: Option<String>,
    },
    Upload,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModpackInstallRequest {
    pub source: ModpackSource,
    #[serde(default)]
    pub keep_extra_content: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ModpackUpdateRequest {
    #[serde(default)]
    pub version_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModpackUnlinkResponse {
    pub unlinked: bool,
    pub adopted_items: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GameVersionChangeDiffType {
    Added,
    Removed,
    Updated,
    ModpackUnlinked,
    GameVersionUpdated,
    LoaderUpdated,
    ConfigFilesUpdated,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GameVersionChangeVersion {
    pub id: String,
    pub version_number: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GameVersionChangeEntry {
    #[serde(rename = "type")]
    pub kind: GameVersionChangeDiffType,
    pub id: Option<crate::model::Id>,
    pub file_name: Option<String>,
    pub project_id: Option<String>,
    pub project_title: Option<String>,
    pub project_icon_url: Option<String>,
    pub current_version: Option<GameVersionChangeVersion>,
    pub new_version: Option<GameVersionChangeVersion>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GameVersionPreviewResponse {
    pub new_game_version: String,
    pub new_loader: LoaderId,
    pub new_loader_version: Option<String>,
    pub has_unknown_content: bool,
    pub changes: Vec<GameVersionChangeEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IncompatiblePolicy {
    UpdateThenDisable,
    Disable,
    Keep,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GameVersionChangeRequest {
    pub game_version: String,
    #[serde(default)]
    pub loader: Option<LoaderId>,
    #[serde(default)]
    pub loader_version: Option<String>,
    pub incompatible_content: IncompatiblePolicy,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub refresh_updates: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PreviewQuery {
    pub game_version: String,
    #[serde(default)]
    pub loader: Option<LoaderId>,
    #[serde(default)]
    pub loader_version: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_skip_reasons_are_spelled_the_way_modrinths_resolver_spells_them() {
        let spellings: Vec<String> = [
            ContentSkipReason::AlreadyInstalled,
            ContentSkipReason::DuplicateProject,
            ContentSkipReason::ConflictingDependency,
            ContentSkipReason::NoCompatibleVersion,
            ContentSkipReason::MissingVersion,
            ContentSkipReason::QuiltFabricApi,
        ]
        .iter()
        .map(|reason| serde_json::to_value(reason).expect("json").as_str().unwrap().to_owned())
        .collect();

        assert_eq!(
            spellings,
            [
                "already_installed",
                "duplicate_project",
                "conflicting_dependency",
                "no_compatible_version",
                "missing_version",
                "quilt_fabric_api"
            ]
        );
    }

    #[test]
    fn a_modpack_source_reads_both_shapes_of_8_10() {
        let from_modrinth: ModpackSource = serde_json::from_value(serde_json::json!({
            "kind": "modrinth", "project_id": "PACK", "version_id": null
        }))
        .expect("the modrinth shape");
        assert!(matches!(from_modrinth, ModpackSource::Modrinth { .. }));

        let uploaded: ModpackSource =
            serde_json::from_value(serde_json::json!({ "kind": "upload" })).expect("the upload shape");
        assert!(matches!(uploaded, ModpackSource::Upload));
    }

    #[test]
    fn a_diff_entry_carries_its_kind_under_the_key_the_layout_reads() {
        let entry = GameVersionChangeEntry {
            kind: GameVersionChangeDiffType::ModpackUnlinked,
            id: None,
            file_name: None,
            project_id: None,
            project_title: None,
            project_icon_url: None,
            current_version: None,
            new_version: None,
        };
        let json = serde_json::to_value(&entry).expect("json");
        assert_eq!(json["type"], "modpack_unlinked");
    }
}
