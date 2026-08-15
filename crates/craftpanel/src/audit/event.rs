use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::model::{AuditAction, Id, JreVendor, LoaderId, Permissions};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddonRef {
    pub project_id: String,
    pub version_id: String,
}

impl AddonRef {
    pub fn new(project_id: impl Into<String>, version_id: impl Into<String>) -> Self {
        Self { project_id: project_id.into(), version_id: version_id.into() }
    }

    fn value(&self) -> Value {
        json!({ "addon_id": self.project_id, "version_id": self.version_id })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModpackSpec {
    Modrinth { project_id: String, version_id: Option<String> },
    LocalFile { filename: String, name: Option<String> },
}

impl ModpackSpec {
    fn value(&self) -> Value {
        match self {
            Self::Modrinth { project_id, version_id } => json!({
                "platform": "modrinth",
                "project_id": project_id,
                "version_id": version_id,
            }),
            Self::LocalFile { filename, name } => json!({
                "platform": "local_file",
                "filename": filename,
                "name": name,
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    ServerCreated,
    ServerReallocated,
    ServerRepaired,
    ServerReset,
    ServerStarted,
    ServerStopped,
    ServerRestarted,
    ServerKilled,
    ConsoleCleared,
    ConsoleCommandExecuted { command: String },
    ChangedServerName { name: String },
    UserInvited { user: Id, permissions: Permissions },
    UserInviteRevoked { user: Id },
    UserPermissionModified { user: Id, permissions: Permissions },
    UserRemoved { user: Id },
    AddonAdded { addons: Vec<AddonRef> },
    AddonUploaded { file_names: Vec<String> },
    AddonDisabled { addons: Vec<AddonRef> },
    AddonEnabled { addons: Vec<AddonRef> },
    AddonDeleted { addons: Vec<AddonRef> },
    AddonUpdated { addons: Vec<AddonRef> },
    ModpackChanged { spec: ModpackSpec },
    ModpackUnlinked { spec: ModpackSpec },
    PortAllocationAdded { port: u16 },
    PortAllocationRemoved { port: u16 },
    LoaderVersionEdited { new_loader: LoaderId, new_version: Option<String> },
    GameVersionEdited { new_version: String },
    ServerPropertiesModified { properties: BTreeMap<String, Option<String>> },
    StartupCommandModified { command: String },
    JavaRuntimeModified { vendor: JreVendor },
    JavaVersionModified { version: u32 },
    FileUploaded { path: String },
    FileDeleted { path: String },
    FileRenamed { from: String, to: String },
    FileEdited { path: String },
    BackupCreated { backup: Id },
    BackupRenamed { backup: Id, from: String, to: String },
    BackupRestored { backup: Id },
    BackupDeleted { backup: Id },
}

impl Event {
    pub fn action(&self) -> AuditAction {
        match self {
            Self::ServerCreated => AuditAction::ServerCreated,
            Self::ServerReallocated => AuditAction::ServerReallocated,
            Self::ServerRepaired => AuditAction::ServerRepaired,
            Self::ServerReset => AuditAction::ServerReset,
            Self::ServerStarted => AuditAction::ServerStarted,
            Self::ServerStopped => AuditAction::ServerStopped,
            Self::ServerRestarted => AuditAction::ServerRestarted,
            Self::ServerKilled => AuditAction::ServerKilled,
            Self::ConsoleCleared => AuditAction::ConsoleCleared,
            Self::ConsoleCommandExecuted { .. } => AuditAction::ConsoleCommandExecuted,
            Self::ChangedServerName { .. } => AuditAction::ChangedServerName,
            Self::UserInvited { .. } => AuditAction::UserInvited,
            Self::UserInviteRevoked { .. } => AuditAction::UserInviteRevoked,
            Self::UserPermissionModified { .. } => AuditAction::UserPermissionModified,
            Self::UserRemoved { .. } => AuditAction::UserRemoved,
            Self::AddonAdded { .. } => AuditAction::AddonAdded,
            Self::AddonUploaded { .. } => AuditAction::AddonUploaded,
            Self::AddonDisabled { .. } => AuditAction::AddonDisabled,
            Self::AddonEnabled { .. } => AuditAction::AddonEnabled,
            Self::AddonDeleted { .. } => AuditAction::AddonDeleted,
            Self::AddonUpdated { .. } => AuditAction::AddonUpdated,
            Self::ModpackChanged { .. } => AuditAction::ModpackChanged,
            Self::ModpackUnlinked { .. } => AuditAction::ModpackUnlinked,
            Self::PortAllocationAdded { .. } => AuditAction::PortAllocationAdded,
            Self::PortAllocationRemoved { .. } => AuditAction::PortAllocationRemoved,
            Self::LoaderVersionEdited { .. } => AuditAction::LoaderVersionEdited,
            Self::GameVersionEdited { .. } => AuditAction::GameVersionEdited,
            Self::ServerPropertiesModified { .. } => AuditAction::ServerPropertiesModified,
            Self::StartupCommandModified { .. } => AuditAction::StartupCommandModified,
            Self::JavaRuntimeModified { .. } => AuditAction::JavaRuntimeModified,
            Self::JavaVersionModified { .. } => AuditAction::JavaVersionModified,
            Self::FileUploaded { .. } => AuditAction::FileUploaded,
            Self::FileDeleted { .. } => AuditAction::FileDeleted,
            Self::FileRenamed { .. } => AuditAction::FileRenamed,
            Self::FileEdited { .. } => AuditAction::FileEdited,
            Self::BackupCreated { .. } => AuditAction::BackupCreated,
            Self::BackupRenamed { .. } => AuditAction::BackupRenamed,
            Self::BackupRestored { .. } => AuditAction::BackupRestored,
            Self::BackupDeleted { .. } => AuditAction::BackupDeleted,
        }
    }

    pub fn metadata(&self) -> Option<Value> {
        let addons = |addons: &Vec<AddonRef>| {
            json!({ "addons": addons.iter().map(AddonRef::value).collect::<Vec<_>>() })
        };

        Some(match self {
            Self::ServerCreated
            | Self::ServerReallocated
            | Self::ServerRepaired
            | Self::ServerReset
            | Self::ServerStarted
            | Self::ServerStopped
            | Self::ServerRestarted
            | Self::ServerKilled
            | Self::ConsoleCleared => return None,
            Self::ConsoleCommandExecuted { command } => json!({ "command": command }),
            Self::ChangedServerName { name } => json!({ "name": name }),
            Self::UserInvited { user, permissions }
            | Self::UserPermissionModified { user, permissions } => json!({
                "user_id": user,
                "permissions": permissions,
            }),
            Self::UserInviteRevoked { user } | Self::UserRemoved { user } => {
                json!({ "user_id": user })
            }
            Self::AddonAdded { addons: list }
            | Self::AddonDisabled { addons: list }
            | Self::AddonEnabled { addons: list }
            | Self::AddonDeleted { addons: list }
            | Self::AddonUpdated { addons: list } => addons(list),
            Self::AddonUploaded { file_names } => json!({ "file_names": file_names }),
            Self::ModpackChanged { spec } | Self::ModpackUnlinked { spec } => {
                json!({ "spec": spec.value() })
            }
            Self::PortAllocationAdded { port } | Self::PortAllocationRemoved { port } => {
                json!({ "port": port })
            }
            Self::LoaderVersionEdited { new_loader, new_version } => json!({
                "new_loader": new_loader.as_str(),
                "new_version": new_version,
            }),
            Self::GameVersionEdited { new_version } => json!({ "new_version": new_version }),
            Self::ServerPropertiesModified { properties } => json!({ "properties": properties }),
            Self::StartupCommandModified { command } => json!({ "command": command }),
            Self::JavaRuntimeModified { vendor } => json!({ "vendor": vendor.as_str() }),
            Self::JavaVersionModified { version } => json!({ "version": version }),
            Self::FileUploaded { path } | Self::FileDeleted { path } | Self::FileEdited { path } => {
                json!({ "path": path })
            }
            Self::FileRenamed { from, to } => json!({ "from": from, "to": to }),
            Self::BackupCreated { backup }
            | Self::BackupRestored { backup }
            | Self::BackupDeleted { backup } => json!({ "id": backup }),
            Self::BackupRenamed { backup, from, to } => {
                json!({ "id": backup, "from": from, "to": to })
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ServerRole;

    #[test]
    fn every_event_carries_the_metadata_its_renderer_demands() {
        let user = Id::new();
        let backup = Id::new();
        let addon = AddonRef::new("AABBCCDD", "11223344");

        let cases: Vec<(Event, Vec<&str>)> = vec![
            (Event::ConsoleCommandExecuted { command: "say hi".into() }, vec!["command"]),
            (Event::ChangedServerName { name: "two".into() }, vec!["name"]),
            (
                Event::UserInvited {
                    user,
                    permissions: Permissions::from_role(ServerRole::Viewer),
                },
                vec!["user_id", "permissions"],
            ),
            (Event::UserRemoved { user }, vec!["user_id"]),
            (Event::AddonAdded { addons: vec![addon.clone()] }, vec!["addons"]),
            (Event::AddonUploaded { file_names: vec!["a.jar".into()] }, vec!["file_names"]),
            (
                Event::ModpackChanged {
                    spec: ModpackSpec::Modrinth {
                        project_id: "p".into(),
                        version_id: Some("v".into()),
                    },
                },
                vec!["spec"],
            ),
            (Event::PortAllocationAdded { port: 25565 }, vec!["port"]),
            (
                Event::LoaderVersionEdited { new_loader: LoaderId::Paper, new_version: None },
                vec!["new_loader", "new_version"],
            ),
            (Event::GameVersionEdited { new_version: "1.21.4".into() }, vec!["new_version"]),
            (
                Event::ServerPropertiesModified { properties: BTreeMap::new() },
                vec!["properties"],
            ),
            (Event::StartupCommandModified { command: "-Xmx".into() }, vec!["command"]),
            (Event::JavaRuntimeModified { vendor: JreVendor::Temurin }, vec!["vendor"]),
            (Event::JavaVersionModified { version: 21 }, vec!["version"]),
            (Event::FileUploaded { path: "mods/a.jar".into() }, vec!["path"]),
            (Event::FileRenamed { from: "a".into(), to: "b".into() }, vec!["from", "to"]),
            (Event::BackupCreated { backup }, vec!["id"]),
            (
                Event::BackupRenamed { backup, from: "a".into(), to: "b".into() },
                vec!["id", "from", "to"],
            ),
        ];

        for (event, keys) in cases {
            let metadata = event.metadata().expect("this action has metadata");
            for key in keys {
                assert!(
                    metadata.get(key).is_some(),
                    "{} would render as an unknown event without {key}: {metadata}",
                    event.action()
                );
            }
        }
    }

    #[test]
    fn the_nine_bare_actions_write_no_metadata() {
        for event in [
            Event::ServerCreated,
            Event::ServerReallocated,
            Event::ServerRepaired,
            Event::ServerReset,
            Event::ServerStarted,
            Event::ServerStopped,
            Event::ServerRestarted,
            Event::ServerKilled,
            Event::ConsoleCleared,
        ] {
            assert_eq!(event.metadata(), None, "{}", event.action());
        }
    }

    #[test]
    fn the_two_fields_with_an_unobvious_wire_type() {
        let java = Event::JavaVersionModified { version: 21 }.metadata().unwrap();
        assert_eq!(java["version"], serde_json::json!(21));

        let invited = Event::UserInvited {
            user: Id::new(),
            permissions: Permissions::from_role(ServerRole::Editor),
        }
        .metadata()
        .unwrap();
        let mask = invited["permissions"].as_str().expect("a string, split('|') needs one");
        assert!(mask.contains("BASE_READ | POWER_ACTIONS"), "{mask}");
    }

    #[test]
    fn a_removed_property_travels_as_null() {
        let mut properties = BTreeMap::new();
        properties.insert("difficulty".to_owned(), Some("hard".to_owned()));
        properties.insert("motd".to_owned(), None);

        let metadata = Event::ServerPropertiesModified { properties }.metadata().unwrap();
        assert_eq!(metadata["properties"]["difficulty"], "hard");
        assert!(metadata["properties"]["motd"].is_null());
    }
}
