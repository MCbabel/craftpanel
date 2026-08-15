#![allow(dead_code)]

use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;
use std::sync::{Mutex, PoisonError};

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use sqlx::sqlite::{Sqlite, SqliteArgumentValue, SqliteTypeInfo, SqliteValueRef};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

#[derive(Debug, thiserror::Error)]
#[error("{value:?} is not a valid {kind}")]
pub struct UnknownValue {
    pub kind: &'static str,
    pub value: String,
}

macro_rules! wire_enum {
    (
        $(#[$outer:meta])*
        pub enum $name:ident {
            $($(#[$inner:meta])* $variant:ident = $text:literal),+ $(,)?
        }
    ) => {
        $(#[$outer])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum $name {
            $($(#[$inner])* $variant),+
        }

        impl $name {
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];
            pub const VALUES: &'static [&'static str] = &[$($text),+];

            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text),+
                }
            }

            #[cfg(test)]
            const fn variant_ident(self) -> &'static str {
                match self {
                    $(Self::$variant => stringify!($variant)),+
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = UnknownValue;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                match text {
                    $($text => Ok(Self::$variant),)+
                    _ => Err(UnknownValue { kind: stringify!($name), value: text.to_owned() }),
                }
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let text = String::deserialize(deserializer)?;
                text.parse().map_err(|_| D::Error::unknown_variant(&text, Self::VALUES))
            }
        }

        impl sqlx::Type<Sqlite> for $name {
            fn type_info() -> SqliteTypeInfo {
                <&str as sqlx::Type<Sqlite>>::type_info()
            }

            fn compatible(ty: &SqliteTypeInfo) -> bool {
                <&str as sqlx::Type<Sqlite>>::compatible(ty)
            }
        }

        impl<'q> sqlx::Encode<'q, Sqlite> for $name {
            fn encode_by_ref(
                &self,
                args: &mut Vec<SqliteArgumentValue<'q>>,
            ) -> Result<IsNull, BoxDynError> {
                args.push(SqliteArgumentValue::Text(Cow::Borrowed(self.as_str())));
                Ok(IsNull::No)
            }
        }

        impl<'r> sqlx::Decode<'r, Sqlite> for $name {
            fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
                Ok(<&str as sqlx::Decode<Sqlite>>::decode(value)?.parse()?)
            }
        }

        #[cfg(test)]
        impl WireEnum for $name {
            const EVERY: &'static [Self] = Self::ALL;

            fn text(self) -> &'static str {
                self.as_str()
            }

            fn ident(self) -> &'static str {
                self.variant_ident()
            }
        }
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Id(ulid::Ulid);

#[derive(Debug, thiserror::Error)]
#[error("{0:?} is not a ULID")]
pub struct InvalidId(String);

impl Id {
    pub fn new() -> Self {
        static CLOCK: Mutex<ulid::Generator> = Mutex::new(ulid::Generator::new());

        let mut clock = CLOCK.lock().unwrap_or_else(PoisonError::into_inner);
        Self(match clock.generate() {
            Ok(id) => id,
            Err(overflow) => overflow.commit_overflow_increment(),
        })
    }
}

impl Default for Id {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0.to_string())
    }
}

impl FromStr for Id {
    type Err = InvalidId;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let ulid = ulid::Ulid::from_string(text).map_err(|_| InvalidId(text.to_owned()))?;
        if text.as_bytes()[0] > b'7' {
            return Err(InvalidId(text.to_owned()));
        }
        Ok(Self(ulid))
    }
}

impl Serialize for Id {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Id {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(D::Error::custom)
    }
}

impl sqlx::Type<Sqlite> for Id {
    fn type_info() -> SqliteTypeInfo {
        <&str as sqlx::Type<Sqlite>>::type_info()
    }

    fn compatible(ty: &SqliteTypeInfo) -> bool {
        <&str as sqlx::Type<Sqlite>>::compatible(ty)
    }
}

impl<'q> sqlx::Encode<'q, Sqlite> for Id {
    fn encode_by_ref(
        &self,
        args: &mut Vec<SqliteArgumentValue<'q>>,
    ) -> Result<IsNull, BoxDynError> {
        args.push(SqliteArgumentValue::Text(Cow::Owned(self.to_string())));
        Ok(IsNull::No)
    }
}

impl<'r> sqlx::Decode<'r, Sqlite> for Id {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        Ok(<&str as sqlx::Decode<Sqlite>>::decode(value)?.parse()?)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(OffsetDateTime);

#[derive(Debug, thiserror::Error)]
#[error("{0:?} is not an RFC 3339 timestamp")]
pub struct InvalidTimestamp(String);

impl Timestamp {
    pub fn now() -> Self {
        Self::at(OffsetDateTime::now_utc())
    }

    pub fn at(moment: OffsetDateTime) -> Self {
        let utc = moment.to_offset(time::UtcOffset::UTC);
        Self(utc.replace_nanosecond(0).expect("zero is a valid nanosecond"))
    }

    pub fn as_datetime(self) -> OffsetDateTime {
        self.0
    }

    pub fn unix_seconds(self) -> i64 {
        self.0.unix_timestamp()
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0.format(&Rfc3339).map_err(|_| fmt::Error)?)
    }
}

impl FromStr for Timestamp {
    type Err = InvalidTimestamp;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        OffsetDateTime::parse(text, &Rfc3339)
            .map(Self::at)
            .map_err(|_| InvalidTimestamp(text.to_owned()))
    }
}

impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(D::Error::custom)
    }
}

impl sqlx::Type<Sqlite> for Timestamp {
    fn type_info() -> SqliteTypeInfo {
        <&str as sqlx::Type<Sqlite>>::type_info()
    }

    fn compatible(ty: &SqliteTypeInfo) -> bool {
        <&str as sqlx::Type<Sqlite>>::compatible(ty)
    }
}

impl<'q> sqlx::Encode<'q, Sqlite> for Timestamp {
    fn encode_by_ref(
        &self,
        args: &mut Vec<SqliteArgumentValue<'q>>,
    ) -> Result<IsNull, BoxDynError> {
        args.push(SqliteArgumentValue::Text(Cow::Owned(self.to_string())));
        Ok(IsNull::No)
    }
}

impl<'r> sqlx::Decode<'r, Sqlite> for Timestamp {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        Ok(<&str as sqlx::Decode<Sqlite>>::decode(value)?.parse()?)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AlwaysFalse;

impl Serialize for AlwaysFalse {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bool(false)
    }
}

impl<'de> Deserialize<'de> for AlwaysFalse {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match bool::deserialize(deserializer)? {
            false => Ok(Self),
            true => Err(D::Error::invalid_value(serde::de::Unexpected::Bool(true), &"false")),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Minecraft;

impl Serialize for Minecraft {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str("Minecraft")
    }
}

impl<'de> Deserialize<'de> for Minecraft {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        if text == "Minecraft" {
            Ok(Self)
        } else {
            Err(D::Error::invalid_value(serde::de::Unexpected::Str(&text), &"Minecraft"))
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(default)]
pub struct OffsetPage {
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

impl OffsetPage {
    pub fn limit(self, fallback: u32, ceiling: u32) -> u32 {
        self.limit.unwrap_or(fallback).clamp(1, ceiling)
    }

    pub fn offset(self) -> u32 {
        self.offset.unwrap_or(0)
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(default)]
pub struct CursorPage {
    pub limit: Option<u32>,
    pub before: Option<Id>,
}

impl CursorPage {
    pub fn limit(self, fallback: u32, ceiling: u32) -> u32 {
        self.limit.unwrap_or(fallback).clamp(1, ceiling)
    }
}

wire_enum! {
    pub enum PanelRole {
        Admin = "admin",
        User = "user",
    }
}

wire_enum! {
    pub enum ServerRole {
        Owner = "owner",
        Editor = "editor",
        Viewer = "viewer",
    }
}

wire_enum! {
    pub enum Permission {
        BaseRead = "BASE_READ",
        PowerActions = "POWER_ACTIONS",
        ExecCommands = "EXEC_COMMANDS",
        FilesWrite = "FILES_WRITE",
        Setup = "SETUP",
        Backups = "BACKUPS",
        Advanced = "ADVANCED",
        ResetServer = "RESET_SERVER",
        ManageUsers = "MANAGE_USERS",
        ServerAdmin = "SERVER_ADMIN",
    }
}

impl Permission {
    pub const fn bits(self) -> u64 {
        match self {
            Self::BaseRead => 1 << 63,
            Self::PowerActions => 1 << 62,
            Self::ExecCommands => 1 << 61,
            Self::FilesWrite => 1 << 60,
            Self::Setup => 1 << 59,
            Self::Backups => 1 << 58,
            Self::Advanced => 1 << 57,
            Self::ResetServer => 1 << 56,
            Self::ManageUsers => 1 << 55,
            Self::ServerAdmin => u64::MAX ^ ((1 << 15) - 1),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Permissions(u64);

impl Permissions {
    pub const NONE: Self = Self(0);

    pub const fn of(permission: Permission) -> Self {
        Self(permission.bits())
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn bits(self) -> u64 {
        self.0
    }

    pub const fn allows(self, permission: Permission) -> bool {
        let wanted = permission.bits();
        self.0 & wanted == wanted
    }

    pub const fn from_role(role: ServerRole) -> Self {
        match role {
            ServerRole::Owner => Self(Permission::ServerAdmin.bits()),
            ServerRole::Editor => Self(
                Permission::BaseRead.bits()
                    | Permission::PowerActions.bits()
                    | Permission::ExecCommands.bits()
                    | Permission::FilesWrite.bits()
                    | Permission::Setup.bits()
                    | Permission::Backups.bits()
                    | Permission::Advanced.bits(),
            ),
            ServerRole::Viewer => {
                Self(Permission::BaseRead.bits() | Permission::PowerActions.bits())
            }
        }
    }

    pub const fn role(self) -> ServerRole {
        if self.allows(Permission::ServerAdmin) {
            return ServerRole::Owner;
        }
        let editing = self.allows(Permission::ExecCommands)
            || self.allows(Permission::FilesWrite)
            || self.allows(Permission::Setup)
            || self.allows(Permission::Backups)
            || self.allows(Permission::Advanced)
            || self.allows(Permission::ResetServer);
        if editing {
            ServerRole::Editor
        } else {
            ServerRole::Viewer
        }
    }
}

impl fmt::Display for Permissions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.allows(Permission::ServerAdmin) {
            return f.write_str(Permission::ServerAdmin.as_str());
        }
        let mut first = true;
        for permission in Permission::ALL {
            if self.0 & permission.bits() == permission.bits() {
                if !first {
                    f.write_str(" | ")?;
                }
                f.write_str(permission.as_str())?;
                first = false;
            }
        }
        Ok(())
    }
}

impl FromStr for Permissions {
    type Err = UnknownValue;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let mut mask = Self::NONE;
        for name in text.split('|').map(str::trim).filter(|name| !name.is_empty()) {
            mask = mask.union(Self::of(name.parse()?));
        }
        Ok(mask)
    }
}

impl Serialize for Permissions {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Permissions {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserRef {
    pub id: Id,
    pub username: String,
    pub avatar_url: Option<String>,
}

wire_enum! {
    pub enum OperationKind {
        ServerCreate = "server_create",
        ServerDelete = "server_delete",
        InstallLoader = "install_loader",
        RepairContent = "repair_content",
        ResetServer = "reset_server",
        InstallModpack = "install_modpack",
        InstallContent = "install_content",
        UpdateContent = "update_content",
        ChangeGameVersion = "change_game_version",
        InstallJava = "install_java",
        BackupCreate = "backup_create",
        BackupRestore = "backup_restore",
        Unarchive = "unarchive",
    }
}

impl OperationKind {
    pub const fn busy_reason(self) -> Option<BusyReasonCode> {
        match self {
            Self::ServerCreate
            | Self::InstallLoader
            | Self::RepairContent
            | Self::ResetServer
            | Self::InstallModpack
            | Self::ChangeGameVersion
            | Self::InstallJava => Some(BusyReasonCode::Installing),
            Self::ServerDelete => Some(BusyReasonCode::Deleting),
            Self::InstallContent | Self::UpdateContent => Some(BusyReasonCode::SyncingContent),
            Self::BackupCreate => Some(BusyReasonCode::BackupCreating),
            Self::BackupRestore => Some(BusyReasonCode::BackupRestoring),
            Self::Unarchive => None,
        }
    }

    pub const fn is_cancellable(self) -> bool {
        matches!(
            self,
            Self::ServerCreate | Self::BackupCreate | Self::BackupRestore | Self::Unarchive
        )
    }

    pub const fn is_retryable(self) -> bool {
        !matches!(
            self,
            Self::Unarchive | Self::ServerDelete | Self::BackupCreate | Self::BackupRestore
        )
    }

    pub const fn allows_running_server(self) -> bool {
        matches!(
            self,
            Self::InstallJava
                | Self::InstallContent
                | Self::UpdateContent
                | Self::BackupCreate
                | Self::Unarchive
        )
    }
}

wire_enum! {
    pub enum OperationState {
        Queued = "queued",
        Ongoing = "ongoing",
        Done = "done",
        Failed = "failed",
        Cancelled = "cancelled",
    }
}

impl OperationState {
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }

    pub const fn is_open(self) -> bool {
        !self.is_terminal()
    }
}

wire_enum! {
    pub enum OperationPhase {
        Analyzing = "analyzing",
        InstallingLoader = "installing_loader",
        Verifying = "verifying",
        RunningInstaller = "running_installer",
        InstallingPack = "installing_pack",
        Addons = "addons",
        WritingConfig = "writing_config",
    }
}

wire_enum! {
    pub enum OperationErrorStep {
        Modloader = "modloader",
        Modpack = "modpack",
        Download = "download",
        Filesystem = "filesystem",
        Internal = "internal",
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationError {
    pub code: String,
    pub message: String,
    pub step: OperationErrorStep,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Operation {
    pub id: Id,
    pub server_id: Id,
    pub kind: OperationKind,
    pub state: OperationState,
    pub phase: Option<OperationPhase>,
    pub progress: f64,
    pub message: Option<String>,
    pub src: Option<String>,
    pub bytes_processed: Option<u64>,
    pub files_processed: Option<u64>,
    pub current_file: Option<String>,
    pub error: Option<OperationError>,
    pub cancellable: bool,
    pub target_id: Option<Id>,
    pub started_by: Option<Id>,
    pub created_at: Timestamp,
    pub started_at: Option<Timestamp>,
    pub finished_at: Option<Timestamp>,
    pub dismissed_at: Option<Timestamp>,
}

wire_enum! {
    pub enum BusyReasonCode {
        Installing = "installing",
        SyncingContent = "syncing_content",
        BackupCreating = "backup_creating",
        BackupRestoring = "backup_restoring",
        Deleting = "deleting",
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OperationAccepted {
    pub operation: Operation,
}

wire_enum! {
    pub enum ServerStatus {
        Installing = "installing",
        Available = "available",
        Broken = "broken",
        Deleting = "deleting",
    }
}

wire_enum! {
    pub enum LoaderId {
        Vanilla = "vanilla",
        Paper = "paper",
        Folia = "folia",
        Purpur = "purpur",
        Leaf = "leaf",
        Fabric = "fabric",
        Velocity = "velocity",
        NeoForge = "neoforge",
        Quilt = "quilt",
        Forge = "forge",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoaderFamily {
    Vanilla,
    Bukkit,
    Modloader,
    Proxy,
}

impl LoaderId {
    pub const fn family(self) -> LoaderFamily {
        match self {
            Self::Vanilla => LoaderFamily::Vanilla,
            Self::Paper | Self::Folia | Self::Purpur | Self::Leaf => LoaderFamily::Bukkit,
            Self::Fabric | Self::Quilt | Self::NeoForge | Self::Forge => LoaderFamily::Modloader,
            Self::Velocity => LoaderFamily::Proxy,
        }
    }

    pub const fn content_type(self) -> ContentProjectType {
        match self.family() {
            LoaderFamily::Vanilla => ContentProjectType::Datapack,
            LoaderFamily::Bukkit | LoaderFamily::Proxy => ContentProjectType::Plugin,
            LoaderFamily::Modloader => ContentProjectType::Mod,
        }
    }

    pub const fn supports_properties(self) -> bool {
        !matches!(self.family(), LoaderFamily::Proxy)
    }

    pub fn source(self) -> Option<crate::loaders::Loader> {
        crate::loaders::Loader::from_id(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerNet {
    pub ip: Option<String>,
    pub port: u16,
    pub domain: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ServerUpstream {
    Modpack { project_id: String, version_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerFlows {
    pub intro: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Server {
    pub id: Id,
    pub name: String,
    pub owner_id: Id,
    pub status: ServerStatus,
    pub game: Minecraft,
    pub loader: Option<LoaderId>,
    pub loader_version: Option<String>,
    pub game_version: Option<String>,
    pub net: ServerNet,
    pub memory_mib: u32,
    pub upstream: Option<ServerUpstream>,
    pub flows: ServerFlows,
    pub backup_quota: u32,
    pub used_backup_quota: u32,
    pub update_channel: UpdateChannel,
    pub current_user_permissions: Permissions,
    pub created_at: Timestamp,
}

wire_enum! {
    pub enum PowerAction {
        Start = "start",
        Stop = "stop",
        Restart = "restart",
        Kill = "kill",
    }
}

wire_enum! {
    pub enum PowerState {
        Stopped = "stopped",
        Starting = "starting",
        Running = "running",
        Stopping = "stopping",
        Crashed = "crashed",
    }
}

wire_enum! {
    pub enum PowerTarget {
        Start = "start",
        Stop = "stop",
        Restart = "restart",
    }
}

wire_enum! {
    pub enum ContentProjectType {
        Mod = "mod",
        Plugin = "plugin",
        Datapack = "datapack",
        Resourcepack = "resourcepack",
        Shader = "shader",
    }
}

wire_enum! {
    pub enum ContentSourceKind {
        Local = "local",
        ModrinthModpack = "modrinth_modpack",
        ServerProject = "server_project",
    }
}

wire_enum! {
    pub enum UpdateChannel {
        Release = "release",
        Beta = "beta",
        Alpha = "alpha",
    }
}

wire_enum! {
    pub enum ModpackSourceKind {
        ModrinthModpack = "modrinth_modpack",
        Local = "local",
    }
}

wire_enum! {
    pub enum ModrinthOwnerKind {
        User = "user",
        Organization = "organization",
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModrinthOwner {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: ModrinthOwnerKind,
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentProject {
    pub id: String,
    pub slug: Option<String>,
    pub title: String,
    pub icon_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentVersion {
    pub id: String,
    pub version_number: String,
    pub file_name: String,
    pub date_published: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiContentItem {
    pub id: Id,
    pub file_name: String,
    pub file_path: String,
    pub size: u64,
    pub enabled: bool,
    pub locked: bool,
    pub project_type: ContentProjectType,
    pub date_added: Timestamp,
    pub source_kind: ContentSourceKind,
    pub environment: Option<String>,
    pub pack_client_retained: AlwaysFalse,
    pub pack_client_depends: bool,
    pub installing: bool,
    pub external: bool,
    pub external_url: Option<String>,
    pub has_update: bool,
    pub update_version_id: Option<String>,
    pub project_id: Option<String>,
    pub project: Option<ContentProject>,
    pub version: Option<ContentVersion>,
    pub owner: Option<ModrinthOwner>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentModpack {
    pub source_kind: ModpackSourceKind,
    pub project_id: Option<String>,
    pub slug: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub icon_url: Option<String>,
    pub filename: Option<String>,
    pub downloads: Option<u64>,
    pub followers: Option<u64>,
    pub owner: Option<ModrinthOwner>,
    pub categories: Vec<String>,
    pub version_id: Option<String>,
    pub version_number: Option<String>,
    pub date_published: Option<Timestamp>,
    pub has_update: bool,
    pub update_version_id: Option<String>,
}

wire_enum! {
    pub enum JreVendor {
        Temurin = "temurin",
        Corretto = "corretto",
        Graal = "graal",
    }
}

pub const KNOWN_PROPERTY_KEYS: [&str; 25] = [
    "allow_cheats",
    "allow_flight",
    "difficulty",
    "enforce_whitelist",
    "force_gamemode",
    "gamemode",
    "generate_structures",
    "generator_settings",
    "hardcore",
    "level_seed",
    "level_type",
    "max_players",
    "max_tick_time",
    "motd",
    "pause_when_empty_seconds",
    "player_idle_timeout",
    "require_resource_pack",
    "resource_pack",
    "resource_pack_id",
    "resource_pack_sha1",
    "simulation_distance",
    "spawn_protection",
    "sync_chunk_writes",
    "view_distance",
    "white_list",
];

macro_rules! known_properties {
    ($($field:ident),+ $(,)?) => {
        #[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(default)]
        pub struct KnownProperties {
            $(
                #[serde(skip_serializing_if = "Option::is_none")]
                pub $field: Option<String>,
            )+
        }

        impl KnownProperties {
            pub fn get(&self, key: &str) -> Option<&str> {
                match key {
                    $(stringify!($field) => self.$field.as_deref(),)+
                    _ => None,
                }
            }

            pub fn set(&mut self, key: &str, value: Option<String>) -> bool {
                match key {
                    $(stringify!($field) => { self.$field = value; true })+
                    _ => false,
                }
            }
        }
    };
}

known_properties!(
    allow_cheats,
    allow_flight,
    difficulty,
    enforce_whitelist,
    force_gamemode,
    gamemode,
    generate_structures,
    generator_settings,
    hardcore,
    level_seed,
    level_type,
    max_players,
    max_tick_time,
    motd,
    pause_when_empty_seconds,
    player_idle_timeout,
    require_resource_pack,
    resource_pack,
    resource_pack_id,
    resource_pack_sha1,
    simulation_distance,
    spawn_protection,
    sync_chunk_writes,
    view_distance,
    white_list,
);

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PropertiesFields {
    pub known: KnownProperties,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<std::collections::BTreeMap<String, String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Allocation {
    pub port: u16,
    pub name: String,
}

wire_enum! {
    pub enum BackupStatus {
        Pending = "pending",
        InProgress = "in_progress",
        TimedOut = "timed_out",
        Error = "error",
        Done = "done",
    }
}

wire_enum! {
    pub enum BackupOperationType {
        Create = "create",
        Restore = "restore",
    }
}

wire_enum! {
    pub enum BackupOperationState {
        Pending = "pending",
        Ongoing = "ongoing",
        Completed = "completed",
        Cancelled = "cancelled",
        Failed = "failed",
        TimedOut = "timed_out",
    }
}

impl BackupOperationType {
    pub const fn kind(self) -> OperationKind {
        match self {
            Self::Create => OperationKind::BackupCreate,
            Self::Restore => OperationKind::BackupRestore,
        }
    }

    pub const fn of(kind: OperationKind) -> Option<Self> {
        match kind {
            OperationKind::BackupCreate => Some(Self::Create),
            OperationKind::BackupRestore => Some(Self::Restore),
            _ => None,
        }
    }
}

impl BackupOperationState {
    pub fn of(state: OperationState, error_code: Option<&str>) -> Self {
        match state {
            OperationState::Queued => Self::Pending,
            OperationState::Ongoing => Self::Ongoing,
            OperationState::Done => Self::Completed,
            OperationState::Cancelled => Self::Cancelled,
            OperationState::Failed if error_code == Some("timeout") => Self::TimedOut,
            OperationState::Failed => Self::Failed,
        }
    }
}

impl BackupStatus {
    pub fn of(newest: BackupOperationState) -> Self {
        match newest {
            BackupOperationState::Pending => Self::Pending,
            BackupOperationState::Ongoing => Self::InProgress,
            BackupOperationState::Completed => Self::Done,
            BackupOperationState::Failed => Self::Error,
            BackupOperationState::TimedOut => Self::TimedOut,
            BackupOperationState::Cancelled => Self::Done,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupOperation {
    pub operation_type: BackupOperationType,
    pub operation_id: Id,
    pub state: BackupOperationState,
    pub scheduled_for: Timestamp,
    pub started_at: Option<Timestamp>,
    pub completed_at: Option<Timestamp>,
    pub has_parent: bool,
    pub error: Option<String>,
    pub should_prompt: bool,
    pub synthetic_legacy: AlwaysFalse,
    pub user_info: Option<UserRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupActiveOperation {
    pub backup_id: Id,
    pub operation_type: BackupOperationType,
    pub operation_id: Id,
    pub has_parent: bool,
    pub scheduled_for: Timestamp,
    pub started_at: Option<Timestamp>,
    pub synthetic_legacy: AlwaysFalse,
    pub user_info: Option<UserRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Backup {
    pub id: Id,
    pub name: String,
    pub created_at: Timestamp,
    pub status: BackupStatus,
    pub locked: AlwaysFalse,
    pub automated: bool,
    pub size_bytes: u64,
    pub history: Vec<BackupOperation>,
    pub location: BackupLocation,
    pub drive_state: Option<DriveFileState>,
    pub drive_web_link: Option<String>,
}

wire_enum! {
    pub enum BackupScheduleStatus {
        Completed = "completed",
        Failed = "failed",
        TimedOut = "timed_out",
        SkippedUnchanged = "skipped_unchanged",
        SkippedLimit = "skipped_limit",
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupSchedule {
    pub enabled: bool,
    pub interval_hours: u32,
    pub hour_utc: u8,
    pub keep_last: u32,
    pub next_run_at: Option<Timestamp>,
    pub last_run_at: Option<Timestamp>,
    pub last_status: Option<BackupScheduleStatus>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerMember {
    pub id: Id,
    pub user: UserRef,
    pub role: ServerRole,
    pub permissions: Permissions,
    pub joined_at: Option<Timestamp>,
    pub invited_at: Timestamp,
    pub last_invite_sent: Option<Timestamp>,
    pub invite_resend_available_at: Option<Timestamp>,
    pub pending: bool,
    pub is_owner: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerRef {
    pub id: Id,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invitation {
    pub id: Id,
    pub server: ServerRef,
    pub role: ServerRole,
    pub invited_by: UserRef,
    pub invited_at: Timestamp,
    pub last_invite_sent: Option<Timestamp>,
}

wire_enum! {
    pub enum AuditAction {
        ServerCreated = "server_created",
        ServerReallocated = "server_reallocated",
        ServerRepaired = "server_repaired",
        ServerReset = "server_reset",
        ServerStarted = "server_started",
        ServerStopped = "server_stopped",
        ServerRestarted = "server_restarted",
        ServerKilled = "server_killed",
        ConsoleCleared = "console_cleared",
        ConsoleCommandExecuted = "console_command_executed",
        ChangedServerName = "changed_server_name",
        UserInvited = "user_invited",
        UserInviteRevoked = "user_invite_revoked",
        UserPermissionModified = "user_permission_modified",
        UserRemoved = "user_removed",
        AddonAdded = "addon_added",
        AddonUploaded = "addon_uploaded",
        AddonDisabled = "addon_disabled",
        AddonEnabled = "addon_enabled",
        AddonDeleted = "addon_deleted",
        AddonUpdated = "addon_updated",
        ModpackChanged = "modpack_changed",
        ModpackUnlinked = "modpack_unlinked",
        PortAllocationAdded = "port_allocation_added",
        PortAllocationRemoved = "port_allocation_removed",
        LoaderVersionEdited = "loader_version_edited",
        GameVersionEdited = "game_version_edited",
        ServerPropertiesModified = "server_properties_modified",
        StartupCommandModified = "startup_command_modified",
        JavaRuntimeModified = "java_runtime_modified",
        JavaVersionModified = "java_version_modified",
        FileUploaded = "file_uploaded",
        FileDeleted = "file_deleted",
        FileRenamed = "file_renamed",
        FileEdited = "file_edited",
        BackupCreated = "backup_created",
        BackupRenamed = "backup_renamed",
        BackupRestored = "backup_restored",
        BackupDeleted = "backup_deleted",
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuditActor {
    User { user_id: Id },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditEvent {
    pub action: AuditAction,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditEntry {
    pub id: Id,
    pub actor: AuditActor,
    pub action: AuditEvent,
    pub server_id: Id,
    pub world_id: Option<Id>,
    pub timestamp: Timestamp,
}

wire_enum! {
    pub enum SystemUserState {
        Provisioning = "provisioning",
        Ready = "ready",
        Error = "error",
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemUser {
    pub state: SystemUserState,
    pub name: String,
    pub uid: Option<u32>,
    pub error_message: Option<String>,
}

wire_enum! {
    pub enum CpuMode {
        Cap = "cap",
        Share = "share",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct UserLimits {
    pub memory_mib: u32,
    pub cpu_mode: CpuMode,
    pub cpu_cores: f64,
    pub pids_max: u32,
    pub disk_mib: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MemoryUsage {
    pub limit_mib: Option<u32>,
    pub allocated_mib: u32,
    pub used_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CpuUsage {
    pub limit_cores: Option<f64>,
    pub used_cores: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PidsUsage {
    pub limit: Option<u32>,
    pub used: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiskUsage {
    pub limit_mib: Option<u32>,
    pub used_bytes: u64,
    pub servers_bytes: u64,
    pub backups_bytes: u64,
    pub complete: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerCounts {
    pub total: u32,
    pub running: u32,
}

wire_enum! {
    pub enum LimitDimension {
        Memory = "memory",
        Cpu = "cpu",
        Pids = "pids",
        Disk = "disk",
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserUsage {
    pub memory: MemoryUsage,
    pub cpu: CpuUsage,
    pub pids: PidsUsage,
    pub disk: DiskUsage,
    pub servers: ServerCounts,
    pub over_limit: bool,
    pub over_limit_dimensions: Vec<LimitDimension>,
    pub measured_at: Timestamp,
}

wire_enum! {
    pub enum BlockedReason {
        OverLimit = "over_limit",
        SystemUserNotReady = "system_user_not_ready",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub can_create_servers: bool,
    pub can_start_servers: bool,
    pub can_manage_panel_users: bool,
    pub blocked_reason: Option<BlockedReason>,
}

wire_enum! {
    pub enum AccountOrigin {
        Admin = "admin",
        Registration = "registration",
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PanelUser {
    pub id: Id,
    pub username: String,
    pub avatar_url: Option<String>,
    pub panel_role: PanelRole,
    pub email: Option<String>,
    pub origin: AccountOrigin,
    pub created_at: Timestamp,
    pub last_login_at: Option<Timestamp>,
    pub must_change_password: bool,
    pub system_user: SystemUser,
    pub limits: Option<UserLimits>,
    pub usage: UserUsage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRef {
    pub id: Id,
    pub expires_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Me {
    #[serde(flatten)]
    pub user: PanelUser,
    pub capabilities: Capabilities,
    pub session: SessionRef,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortRange {
    pub from: u16,
    pub to: u16,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PanelSettings {
    pub public_address: Option<String>,
    pub port_pool: PortRange,
    pub default_limits: UserLimits,
    pub max_upload_bytes: u64,
    pub max_backups_per_server: u32,
    pub external_services_enabled: bool,
    pub max_concurrent_operations: u32,
    pub stop_grace_seconds: u32,
    pub registration_enabled: bool,
    pub registration_requires_approval: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthOptions {
    pub registration_enabled: bool,
    pub registration_requires_approval: bool,
    pub password_reset_enabled: bool,
}

wire_enum! {
    pub enum RegistrationState {
        EmailUnverified = "email_unverified",
        AwaitingApproval = "awaiting_approval",
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Registration {
    pub id: Id,
    pub username: String,
    pub email: String,
    pub state: RegistrationState,
    pub signup_ip: Option<String>,
    pub created_at: Timestamp,
    pub verified_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegistrationList {
    pub registrations: Vec<Registration>,
    pub total: u32,
}

wire_enum! {
    pub enum DriveAccountState {
        Connected = "connected",
        Revoked = "revoked",
        Error = "error",
    }
}

wire_enum! {
    pub enum DriveLinkState {
        Waiting = "waiting",
        Accepted = "accepted",
        Denied = "denied",
        Expired = "expired",
    }
}

wire_enum! {
    pub enum BackupLocation {
        Local = "local",
        Drive = "drive",
    }
}

wire_enum! {
    pub enum DriveFileState {
        Present = "present",
        Missing = "missing",
        Trashed = "trashed",
        Unreachable = "unreachable",
    }
}

wire_enum! {
    pub enum BackupTargetPolicy {
        UserChoice = "user_choice",
        DriveOnly = "drive_only",
        LocalOnly = "local_only",
    }
}

wire_enum! {
    pub enum BackupTargetReason {
        Ok = "ok",
        NotConfigured = "not_configured",
        NotConnected = "not_connected",
        Policy = "policy",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupTarget {
    pub target: BackupLocation,
    pub effective_target: BackupLocation,
    pub policy: BackupTargetPolicy,
    pub reason: BackupTargetReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct UpdateBackupTargetRequest {
    pub target: BackupLocation,
}

#[cfg(test)]
trait WireEnum: Copy + 'static {
    const EVERY: &'static [Self];

    fn text(self) -> &'static str;
    fn ident(self) -> &'static str;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snake_case(ident: &str) -> String {
        let mut out = String::new();
        for (index, letter) in ident.char_indices() {
            if letter.is_ascii_uppercase() {
                if index > 0 {
                    out.push('_');
                }
                out.push(letter.to_ascii_lowercase());
            } else {
                out.push(letter);
            }
        }
        out
    }

    fn assert_snake_case<T: WireEnum + fmt::Debug>(exceptions: &[&str]) {
        for value in T::EVERY {
            if exceptions.contains(&value.ident()) {
                continue;
            }
            assert_eq!(
                value.text(),
                snake_case(value.ident()),
                "{:?} does not spell its variant in snake_case",
                value
            );
        }
    }

    fn assert_round_trip<T>(value: &T)
    where
        T: Serialize + serde::de::DeserializeOwned + PartialEq + fmt::Debug,
    {
        let text = serde_json::to_string(value).expect("serialises");
        let back: T = serde_json::from_str(&text).unwrap_or_else(|err| {
            panic!("{text} does not read back: {err}");
        });
        assert_eq!(&back, value, "round trip changed the value: {text}");
    }

    fn from_contract<T: serde::de::DeserializeOwned>(json: &str) -> T {
        serde_json::from_str(json).expect("the contract's own example must parse")
    }

    const OPERATION_ID: &str = "01JZ8QK3F0V6WQ0X6M2N9CQ7RT";
    const SERVER_ID: &str = "01JZ8QJ9T4YB1S8HK4P0ZDA3WM";
    const USER_ID: &str = "01JZ8Q9V0RS2H5PT7YF3D1XKAE";

    fn id(text: &str) -> Id {
        text.parse().expect("a ULID from the contract")
    }

    fn stamp(text: &str) -> Timestamp {
        text.parse().expect("a timestamp from the contract")
    }

    fn user_ref() -> UserRef {
        UserRef { id: id(USER_ID), username: "max".to_owned(), avatar_url: None }
    }

    fn operation() -> Operation {
        Operation {
            id: id(OPERATION_ID),
            server_id: id(SERVER_ID),
            kind: OperationKind::Unarchive,
            state: OperationState::Ongoing,
            phase: None,
            progress: 0.42,
            message: Some("Extracting archive".to_owned()),
            src: Some("/plugins/pack.zip".to_owned()),
            bytes_processed: Some(18_874_368),
            files_processed: Some(91),
            current_file: Some("plugins/EssentialsX/config.yml".to_owned()),
            error: None,
            cancellable: true,
            target_id: None,
            started_by: Some(id(USER_ID)),
            created_at: stamp("2026-08-12T14:03:11Z"),
            started_at: Some(stamp("2026-08-12T14:03:11Z")),
            finished_at: None,
            dismissed_at: None,
        }
    }

    fn server() -> Server {
        Server {
            id: id(SERVER_ID),
            name: "Survival".to_owned(),
            owner_id: id(USER_ID),
            status: ServerStatus::Available,
            game: Minecraft,
            loader: Some(LoaderId::Paper),
            loader_version: Some("45".to_owned()),
            game_version: Some("1.21.8".to_owned()),
            net: ServerNet {
                ip: Some("192.0.2.10".to_owned()),
                port: 25565,
                domain: String::new(),
            },
            memory_mib: 4096,
            upstream: None,
            flows: ServerFlows { intro: false },
            backup_quota: 10,
            used_backup_quota: 2,
            update_channel: UpdateChannel::Release,
            current_user_permissions: Permissions::from_role(ServerRole::Owner),
            created_at: stamp("2026-08-01T14:22:03Z"),
        }
    }

    fn limits() -> UserLimits {
        UserLimits {
            memory_mib: 8192,
            cpu_mode: CpuMode::Cap,
            cpu_cores: 4.0,
            pids_max: 512,
            disk_mib: 51200,
        }
    }

    fn usage() -> UserUsage {
        UserUsage {
            memory: MemoryUsage {
                limit_mib: Some(8192),
                allocated_mib: 10240,
                used_bytes: 3221225472,
            },
            cpu: CpuUsage { limit_cores: Some(4.0), used_cores: 1.24 },
            pids: PidsUsage { limit: Some(512), used: 137 },
            disk: DiskUsage {
                limit_mib: Some(51200),
                used_bytes: 9663676416,
                servers_bytes: 8589934592,
                backups_bytes: 1073741824,
                complete: true,
            },
            servers: ServerCounts { total: 3, running: 1 },
            over_limit: true,
            over_limit_dimensions: vec![LimitDimension::Memory],
            measured_at: stamp("2026-08-12T14:03:11Z"),
        }
    }

    fn panel_user() -> PanelUser {
        PanelUser {
            id: id(USER_ID),
            username: "max".to_owned(),
            avatar_url: None,
            panel_role: PanelRole::User,
            email: Some("max@example.test".to_owned()),
            origin: AccountOrigin::Registration,
            created_at: stamp("2026-07-01T09:12:44Z"),
            last_login_at: Some(stamp("2026-08-12T13:58:02Z")),
            must_change_password: false,
            system_user: SystemUser {
                state: SystemUserState::Ready,
                name: format!("craft-{}", USER_ID.to_lowercase()),
                uid: Some(6104),
                error_message: None,
            },
            limits: Some(limits()),
            usage: usage(),
        }
    }

    fn backup() -> Backup {
        Backup {
            id: id("01JEXZ9K2QW8T7VN4M0P3RCB6D"),
            name: "Before the nether rebuild".to_owned(),
            created_at: stamp("2026-08-12T14:03:11Z"),
            status: BackupStatus::InProgress,
            locked: AlwaysFalse,
            automated: false,
            size_bytes: 0,
            location: BackupLocation::Local,
            drive_state: None,
            drive_web_link: None,
            history: vec![BackupOperation {
                operation_type: BackupOperationType::Create,
                operation_id: id("01JEXZ9K2QW8T7VN4M0P3RCB6E"),
                state: BackupOperationState::Ongoing,
                scheduled_for: stamp("2026-08-12T14:03:11Z"),
                started_at: Some(stamp("2026-08-12T14:03:12Z")),
                completed_at: None,
                has_parent: false,
                error: None,
                should_prompt: true,
                synthetic_legacy: AlwaysFalse,
                user_info: Some(user_ref()),
            }],
        }
    }

    fn content_item() -> ApiContentItem {
        ApiContentItem {
            id: id("01JEXQ7A5B9C1D3E5F7G9H1J3K"),
            file_name: "sodium-0.6.0.jar".to_owned(),
            file_path: "/mods/sodium-0.6.0.jar".to_owned(),
            size: 1_234_567,
            enabled: true,
            locked: false,
            project_type: ContentProjectType::Mod,
            date_added: stamp("2026-08-12T14:03:11Z"),
            source_kind: ContentSourceKind::Local,
            environment: None,
            pack_client_retained: AlwaysFalse,
            pack_client_depends: false,
            installing: false,
            external: false,
            external_url: None,
            has_update: false,
            update_version_id: None,
            project_id: None,
            project: None,
            version: None,
            owner: None,
        }
    }

    fn member() -> ServerMember {
        ServerMember {
            id: id("01K2FA1N2P3Q4R5S6T7V8W9X0Y"),
            user: user_ref(),
            role: ServerRole::Editor,
            permissions: Permissions::from_role(ServerRole::Editor),
            joined_at: Some(stamp("2026-07-05T08:01:44Z")),
            invited_at: stamp("2026-07-04T19:00:00Z"),
            last_invite_sent: Some(stamp("2026-07-04T19:00:00Z")),
            invite_resend_available_at: None,
            pending: false,
            is_owner: false,
        }
    }

    #[test]
    fn every_wire_name_is_the_snake_case_of_its_variant() {
        assert_snake_case::<PanelRole>(&[]);
        assert_snake_case::<ServerRole>(&[]);
        assert_snake_case::<OperationKind>(&[]);
        assert_snake_case::<OperationState>(&[]);
        assert_snake_case::<OperationPhase>(&[]);
        assert_snake_case::<OperationErrorStep>(&[]);
        assert_snake_case::<BusyReasonCode>(&[]);
        assert_snake_case::<ServerStatus>(&[]);
        assert_snake_case::<PowerAction>(&[]);
        assert_snake_case::<PowerState>(&[]);
        assert_snake_case::<PowerTarget>(&[]);
        assert_snake_case::<JreVendor>(&[]);
        assert_snake_case::<ContentProjectType>(&[]);
        assert_snake_case::<ContentSourceKind>(&[]);
        assert_snake_case::<ModpackSourceKind>(&[]);
        assert_snake_case::<ModrinthOwnerKind>(&[]);
        assert_snake_case::<UpdateChannel>(&[]);
        assert_snake_case::<BackupStatus>(&[]);
        assert_snake_case::<BackupOperationType>(&[]);
        assert_snake_case::<BackupOperationState>(&[]);
        assert_snake_case::<BackupScheduleStatus>(&[]);
        assert_snake_case::<AuditAction>(&[]);
        assert_snake_case::<SystemUserState>(&[]);
        assert_snake_case::<CpuMode>(&[]);
        assert_snake_case::<LimitDimension>(&[]);
        assert_snake_case::<BlockedReason>(&[]);

        assert_snake_case::<LoaderId>(&["NeoForge"]);
        assert_eq!(LoaderId::NeoForge.as_str(), "neoforge");
    }

    #[test]
    fn the_audit_catalogue_has_the_thirty_nine_names_the_parser_renders() {
        assert_eq!(AuditAction::ALL.len(), 39);
        assert!(AuditAction::from_str("changed_server_subdomain").is_err());
        assert!(AuditAction::from_str("sftp_login").is_err());
        assert!(AuditAction::from_str("server_plan_changed").is_err());
    }

    #[test]
    fn the_loader_catalogue_has_ten_entries_and_seven_sources() {
        assert_eq!(LoaderId::ALL.len(), 10);
        let with_source = LoaderId::ALL.iter().filter(|l| l.source().is_some()).count();
        assert_eq!(with_source, 7);
        assert!(LoaderId::Forge.source().is_none());
        assert_eq!(LoaderId::Paper.source(), Some(crate::loaders::Loader::Paper));
    }

    #[test]
    fn a_ulid_survives_the_round_trip_and_rubbish_does_not() {
        let text = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
        assert_eq!(id(text).to_string(), text);

        for rubbish in [
            "",
            "not-a-ulid",
            "01ARZ3NDEKTSV4RRFFQ69G5FA",
            "01ARZ3NDEKTSV4RRFFQ69G5FAVV",
            "01ARZ3NDEKTSV4RRFFQ69G5FAI",
            "01ARZ3NDEKTSV4RRFFQ69G5FAU",
            "ZZZZZZZZZZZZZZZZZZZZZZZZZZ",
            "01ARZ3NDEKTSV4RRFFQ69G5FA;",
            " 01ARZ3NDEKTSV4RRFFQ69G5FAV",
        ] {
            assert!(rubbish.parse::<Id>().is_err(), "{rubbish:?} must not pass for a ULID");
        }
    }

    #[test]
    fn an_invalid_ulid_cannot_arrive_through_json_either() {
        assert!(serde_json::from_str::<Id>("\"01ARZ3NDEKTSV4RRFFQ69G5FAV\"").is_ok());
        assert!(serde_json::from_str::<Id>("\"nonsense\"").is_err());
        assert!(serde_json::from_str::<Id>("42").is_err());

        let generated = Id::new();
        assert_eq!(generated.to_string().len(), 26);
        assert_eq!(generated.to_string().parse::<Id>().unwrap(), generated);
    }

    #[test]
    fn a_generated_id_is_a_name_the_root_helper_accepts() {
        for _ in 0..64 {
            let id = Id::new().to_string();
            assert!(craftpanel_proto::is_valid_user_id(&id), "the helper refuses {id}");
        }
    }

    #[test]
    fn ids_sort_the_way_the_before_cursor_expects() {
        let older = Id::new();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let newer = Id::new();
        assert!(newer > older);
        assert!(newer.to_string() > older.to_string());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn ids_made_at_once_inside_one_millisecond_sort_in_the_order_they_were_made() {
        let made = std::sync::Arc::new(Mutex::new(Vec::new()));
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let made = std::sync::Arc::clone(&made);
            tasks.push(tokio::spawn(async move {
                for _ in 0..500 {
                    {
                        let mut order = made.lock().expect("the list of ids");
                        order.push(Id::new());
                    }
                    tokio::task::yield_now().await;
                }
            }));
        }
        for task in tasks {
            task.await.expect("a task");
        }

        let made = made.lock().expect("the list of ids");
        assert_eq!(made.len(), 4000);
        for (index, pair) in made.windows(2).enumerate() {
            assert!(
                pair[0] < pair[1] && pair[0].to_string() < pair[1].to_string(),
                "id {index} was made before the next one and does not sort before it: \
                 {} then {}",
                pair[0],
                pair[1]
            );
        }

        let (mut crowd, mut busiest) = (1, 1);
        for pair in made.windows(2) {
            crowd =
                if pair[0].0.timestamp_ms() == pair[1].0.timestamp_ms() { crowd + 1 } else { 1 };
            busiest = busiest.max(crowd);
        }
        assert!(busiest >= 100, "the busiest millisecond held {busiest} of the 4000 ids");
    }

    #[test]
    fn a_timestamp_is_written_the_way_the_contract_writes_it() {
        let stamp: Timestamp = "2026-08-12T14:03:11Z".parse().unwrap();
        assert_eq!(stamp.to_string(), "2026-08-12T14:03:11Z");
        assert_eq!(serde_json::to_string(&stamp).unwrap(), "\"2026-08-12T14:03:11Z\"");

        let shifted: Timestamp = "2026-08-12T16:03:11.480+02:00".parse().unwrap();
        assert_eq!(shifted.to_string(), "2026-08-12T14:03:11Z");
        assert_eq!(shifted, stamp);
        assert_eq!(Timestamp::now().to_string().len(), "2026-08-12T14:03:11Z".len());
        assert_eq!(stamp.unix_seconds(), 1_786_543_391);

        for rubbish in ["", "2026-08-12", "2026-08-12 14:03:11", "12.08.2026", "1755007391"] {
            assert!(rubbish.parse::<Timestamp>().is_err(), "{rubbish:?} is not RFC 3339");
        }
    }

    #[test]
    fn timestamps_written_as_text_sort_by_time() {
        let mut written = ["2026-08-12T14:03:11Z", "2025-12-31T23:59:59Z", "2026-08-12T09:00:00Z"]
            .map(|text| stamp(text).to_string());
        written.sort();
        assert_eq!(written[0], "2025-12-31T23:59:59Z");
        assert_eq!(written[2], "2026-08-12T14:03:11Z");
    }

    #[test]
    fn the_permission_mask_is_one_string_with_the_bit_names_in_it() {
        let editor = Permissions::from_role(ServerRole::Editor);
        assert_eq!(
            editor.to_string(),
            "BASE_READ | POWER_ACTIONS | EXEC_COMMANDS | FILES_WRITE | SETUP | BACKUPS | ADVANCED"
        );
        let viewer = Permissions::from_role(ServerRole::Viewer);
        assert_eq!(viewer.to_string(), "BASE_READ | POWER_ACTIONS");
        assert_eq!(Permissions::from_role(ServerRole::Owner).to_string(), "SERVER_ADMIN");
        assert_eq!(Permissions::NONE.to_string(), "");

        for role in ServerRole::ALL {
            let mask = Permissions::from_role(*role);
            assert_eq!(mask.to_string().parse::<Permissions>().unwrap(), mask);
            assert_round_trip(&mask);
        }
        assert_eq!("".parse::<Permissions>().unwrap(), Permissions::NONE);
        assert!("BASE_READ | FLY_TO_THE_MOON".parse::<Permissions>().is_err());
    }

    #[test]
    fn the_roles_grant_exactly_the_rights_the_matrix_promises() {
        let viewer = Permissions::from_role(ServerRole::Viewer);
        let editor = Permissions::from_role(ServerRole::Editor);
        let owner = Permissions::from_role(ServerRole::Owner);

        assert!(viewer.allows(Permission::BaseRead));
        assert!(viewer.allows(Permission::PowerActions));
        for denied in [
            Permission::ExecCommands,
            Permission::FilesWrite,
            Permission::Setup,
            Permission::Backups,
            Permission::Advanced,
            Permission::ResetServer,
            Permission::ManageUsers,
            Permission::ServerAdmin,
        ] {
            assert!(!viewer.allows(denied), "a viewer must not hold {denied}");
        }

        assert!(editor.allows(Permission::FilesWrite));
        assert!(editor.allows(Permission::Backups));
        assert!(!editor.allows(Permission::ResetServer));
        assert!(!editor.allows(Permission::ManageUsers));

        let admin = Permission::ServerAdmin.bits();
        for permission in Permission::ALL {
            assert!(owner.allows(*permission), "an owner must hold {permission}");
            let held = admin & permission.bits();
            assert_eq!(held, permission.bits(), "{permission} sits outside SERVER_ADMIN");
        }
    }

    #[test]
    fn the_role_can_be_read_back_out_of_the_bits() {
        for role in ServerRole::ALL {
            assert_eq!(Permissions::from_role(*role).role(), *role);
        }
        assert_eq!(Permissions::of(Permission::ResetServer).role(), ServerRole::Editor);
        assert_eq!(Permissions::of(Permission::BaseRead).role(), ServerRole::Viewer);
        assert_eq!(Permissions::NONE.role(), ServerRole::Viewer);
    }

    #[test]
    fn every_kind_of_run_carries_the_lock_that_table_five_eight_gives_it() {
        assert_eq!(OperationKind::Unarchive.busy_reason(), None);
        for kind in OperationKind::ALL {
            if *kind != OperationKind::Unarchive {
                assert!(kind.busy_reason().is_some(), "{kind} locks nothing");
            }
        }
        assert_eq!(OperationKind::InstallJava.busy_reason(), Some(BusyReasonCode::Installing));
        assert_eq!(
            OperationKind::UpdateContent.busy_reason(),
            Some(BusyReasonCode::SyncingContent)
        );
        assert_eq!(OperationKind::ServerDelete.busy_reason(), Some(BusyReasonCode::Deleting));

        let cancellable: Vec<_> =
            OperationKind::ALL.iter().filter(|k| k.is_cancellable()).copied().collect();
        assert_eq!(
            cancellable,
            [
                OperationKind::ServerCreate,
                OperationKind::BackupCreate,
                OperationKind::BackupRestore,
                OperationKind::Unarchive
            ]
        );
        assert!(!OperationKind::InstallLoader.is_cancellable());
        assert!(!OperationKind::BackupCreate.is_retryable());
        assert!(!OperationKind::Unarchive.is_retryable());
        assert!(OperationKind::InstallLoader.is_retryable());

        assert!(OperationKind::BackupCreate.allows_running_server());
        assert!(!OperationKind::BackupRestore.allows_running_server());
        assert!(!OperationKind::ServerDelete.allows_running_server());

        assert!(OperationState::Queued.is_open());
        assert!(!OperationState::Cancelled.is_open());
        assert!(OperationState::Cancelled.is_terminal());
    }

    #[test]
    fn a_backup_run_reads_as_the_operation_it_is() {
        assert_eq!(
            BackupOperationState::of(OperationState::Queued, None),
            BackupOperationState::Pending
        );
        assert_eq!(
            BackupOperationState::of(OperationState::Done, None),
            BackupOperationState::Completed
        );
        assert_eq!(
            BackupOperationState::of(OperationState::Failed, Some("no_space")),
            BackupOperationState::Failed
        );
        assert_eq!(
            BackupOperationState::of(OperationState::Failed, Some("timeout")),
            BackupOperationState::TimedOut
        );

        assert_eq!(BackupOperationType::Create.kind(), OperationKind::BackupCreate);
        assert_eq!(BackupOperationType::of(OperationKind::BackupRestore),
                   Some(BackupOperationType::Restore));
        assert_eq!(BackupOperationType::of(OperationKind::Unarchive), None);

        assert_eq!(BackupStatus::of(BackupOperationState::Ongoing), BackupStatus::InProgress);
        assert_eq!(BackupStatus::of(BackupOperationState::Completed), BackupStatus::Done);
        assert_eq!(BackupStatus::of(BackupOperationState::TimedOut), BackupStatus::TimedOut);
        assert_eq!(BackupStatus::of(BackupOperationState::Failed), BackupStatus::Error);
    }

    #[test]
    fn a_loader_knows_its_family_and_what_the_content_tab_calls_its_files() {
        assert_eq!(LoaderId::Paper.family(), LoaderFamily::Bukkit);
        assert_eq!(LoaderId::Leaf.family(), LoaderFamily::Bukkit);
        assert_eq!(LoaderId::Quilt.family(), LoaderFamily::Modloader);
        assert_ne!(LoaderId::Fabric.family(), LoaderId::Paper.family());

        assert_eq!(LoaderId::Vanilla.content_type(), ContentProjectType::Datapack);
        assert_eq!(LoaderId::Velocity.content_type(), ContentProjectType::Plugin);
        assert_eq!(LoaderId::Purpur.content_type(), ContentProjectType::Plugin);
        assert_eq!(LoaderId::NeoForge.content_type(), ContentProjectType::Mod);

        assert!(!LoaderId::Velocity.supports_properties());
        assert!(LoaderId::Vanilla.supports_properties());
    }

    #[test]
    fn the_fields_the_contract_nails_down_cannot_be_written_otherwise() {
        assert_eq!(serde_json::to_string(&AlwaysFalse).unwrap(), "false");
        assert!(serde_json::from_str::<AlwaysFalse>("false").is_ok());
        assert!(serde_json::from_str::<AlwaysFalse>("true").is_err());

        assert_eq!(serde_json::to_string(&Minecraft).unwrap(), "\"Minecraft\"");
        assert!(serde_json::from_str::<Minecraft>("\"Terraria\"").is_err());

        let json = serde_json::to_value(backup()).unwrap();
        assert_eq!(json["locked"], serde_json::json!(false));
        assert_eq!(json["history"][0]["synthetic_legacy"], serde_json::json!(false));
        assert_eq!(serde_json::to_value(content_item()).unwrap()["pack_client_retained"], false);
        assert_eq!(serde_json::to_value(server()).unwrap()["game"], "Minecraft");
    }

    #[test]
    fn the_known_properties_are_the_twenty_five_the_page_renders() {
        assert_eq!(KNOWN_PROPERTY_KEYS.len(), 25);

        let mut filled = KnownProperties::default();
        for key in KNOWN_PROPERTY_KEYS {
            assert!(filled.set(key, Some(format!("value of {key}"))), "{key} has no field");
        }
        assert!(!filled.set("enable-command-block", Some("true".to_owned())));

        let json = serde_json::to_value(&filled).unwrap();
        let written: Vec<&String> = json.as_object().unwrap().keys().collect();
        assert_eq!(written.len(), 25);
        for key in KNOWN_PROPERTY_KEYS {
            assert_eq!(filled.get(key), Some(format!("value of {key}").as_str()));
            assert!(json.get(key).is_some(), "{key} is missing from the JSON");
        }

        let empty = serde_json::to_value(KnownProperties::default()).unwrap();
        assert_eq!(empty.as_object().unwrap().len(), 0);
        assert_round_trip(&filled);
    }

    #[test]
    fn every_shared_type_survives_rust_to_json_and_back() {
        assert_round_trip(&user_ref());
        assert_round_trip(&operation());
        assert_round_trip(&OperationAccepted { operation: operation() });
        assert_round_trip(&server());
        assert_round_trip(&ServerUpstream::Modpack {
            project_id: "1KVo5zza".to_owned(),
            version_id: "8xQZ4rTt".to_owned(),
        });
        assert_round_trip(&Allocation { port: 25566, name: "Query".to_owned() });
        assert_round_trip(&content_item());
        assert_round_trip(&backup());
        assert_round_trip(&BackupActiveOperation {
            backup_id: id("01JEXZ9K2QW8T7VN4M0P3RCB6D"),
            operation_type: BackupOperationType::Create,
            operation_id: id("01JEXZ9K2QW8T7VN4M0P3RCB6E"),
            has_parent: false,
            scheduled_for: stamp("2026-08-12T14:03:11Z"),
            started_at: Some(stamp("2026-08-12T14:03:12Z")),
            synthetic_legacy: AlwaysFalse,
            user_info: Some(user_ref()),
        });
        assert_round_trip(&BackupSchedule {
            enabled: true,
            interval_hours: 24,
            hour_utc: 4,
            keep_last: 7,
            next_run_at: Some(stamp("2026-08-13T04:00:00Z")),
            last_run_at: None,
            last_status: Some(BackupScheduleStatus::SkippedUnchanged),
            last_error: None,
        });
        assert_round_trip(&member());
        assert_round_trip(&Invitation {
            id: id("01K2FA2Y3Z4A5B6C7D8E9F0G1H"),
            server: ServerRef { id: id(SERVER_ID), name: "Survival".to_owned() },
            role: ServerRole::Viewer,
            invited_by: user_ref(),
            invited_at: stamp("2026-08-12T13:59:00Z"),
            last_invite_sent: Some(stamp("2026-08-12T14:05:30Z")),
        });
        assert_round_trip(&AuditEntry {
            id: id("01K2FA0B1C2D3E4F5G6H7J8K9M"),
            actor: AuditActor::User { user_id: id(USER_ID) },
            action: AuditEvent {
                action: AuditAction::ConsoleCommandExecuted,
                metadata: Some(serde_json::json!({ "command": "say hello" })),
            },
            server_id: id(SERVER_ID),
            world_id: None,
            timestamp: stamp("2026-08-12T14:03:11Z"),
        });
        assert_round_trip(&panel_user());
        assert_round_trip(&Me {
            user: panel_user(),
            capabilities: Capabilities {
                can_create_servers: false,
                can_start_servers: false,
                can_manage_panel_users: false,
                blocked_reason: Some(BlockedReason::OverLimit),
            },
            session: SessionRef {
                id: id("01K2G9ZZ0A1B2C3D4E5F6G7H8J"),
                expires_at: stamp("2026-09-11T13:58:02Z"),
            },
        });
        assert_round_trip(&PanelSettings {
            public_address: Some("mc.example.org".to_owned()),
            port_pool: PortRange { from: 25565, to: 25700 },
            default_limits: limits(),
            max_upload_bytes: 4 * 1024 * 1024 * 1024,
            max_backups_per_server: 10,
            external_services_enabled: true,
            max_concurrent_operations: 2,
            stop_grace_seconds: 60,
            registration_enabled: false,
            registration_requires_approval: true,
        });
        assert_round_trip(&ContentModpack {
            source_kind: ModpackSourceKind::ModrinthModpack,
            project_id: Some("1KVo5zza".to_owned()),
            slug: Some("fabulously-optimized".to_owned()),
            title: "Fabulously Optimized".to_owned(),
            description: None,
            icon_url: None,
            filename: None,
            downloads: Some(12),
            followers: None,
            owner: Some(ModrinthOwner {
                id: "abc".to_owned(),
                name: "someone".to_owned(),
                kind: ModrinthOwnerKind::Organization,
                avatar_url: None,
            }),
            categories: vec!["optimization".to_owned()],
            version_id: Some("8xQZ4rTt".to_owned()),
            version_number: Some("6.4.0".to_owned()),
            date_published: Some(stamp("2026-08-01T00:00:00Z")),
            has_update: false,
            update_version_id: None,
        });
    }

    #[test]
    fn the_operation_the_contract_prints_reads_as_the_operation_we_declare() {
        let json = r#"{
          "id": "01JZ8QK3F0V6WQ0X6M2N9CQ7RT",
          "server_id": "01JZ8QJ9T4YB1S8HK4P0ZDA3WM",
          "kind": "unarchive",
          "state": "ongoing",
          "phase": null,
          "progress": 0.42,
          "message": "Extracting archive",
          "src": "/plugins/pack.zip",
          "bytes_processed": 18874368,
          "files_processed": 91,
          "current_file": "plugins/EssentialsX/config.yml",
          "error": null,
          "cancellable": true,
          "target_id": null,
          "started_by": "01JZ8Q9V0RS2H5PT7YF3D1XKAE",
          "created_at": "2026-08-12T14:03:11Z",
          "started_at": "2026-08-12T14:03:11Z",
          "finished_at": null,
          "dismissed_at": null
        }"#;

        assert_eq!(from_contract::<Operation>(json), operation());
    }

    #[test]
    fn a_finished_run_from_the_contract_reads_back_too() {
        let json = r#"{
          "id": "01JZ8QM7A2E4N7T1V5C8H3RGPD",
          "server_id": "01JZ8QJ9T4YB1S8HK4P0ZDA3WM",
          "kind": "server_create",
          "state": "done",
          "phase": null,
          "progress": 1,
          "message": "Server ready",
          "src": null,
          "bytes_processed": 47185920,
          "files_processed": null,
          "current_file": null,
          "error": null,
          "cancellable": false,
          "target_id": null,
          "started_by": "01JZ8Q9V0RS2H5PT7YF3D1XKAE",
          "created_at": "2026-08-12T14:00:02Z",
          "started_at": "2026-08-12T14:00:02Z",
          "finished_at": "2026-08-12T14:01:44Z",
          "dismissed_at": "2026-08-12T14:01:44Z"
        }"#;

        let parsed: Operation = from_contract(json);
        assert_eq!(parsed.kind, OperationKind::ServerCreate);
        assert!(parsed.state.is_terminal());
        assert_eq!(parsed.progress, 1.0);
        assert_eq!(parsed.finished_at, Some(stamp("2026-08-12T14:01:44Z")));
        assert_round_trip(&parsed);
    }

    #[test]
    fn the_member_the_contract_prints_reads_as_the_member_we_declare() {
        let json = r#"{
          "id": "01K2FA2Y3Z4A5B6C7D8E9F0G1H",
          "user": { "id": "01K2F82X3Y4Z5A6B7C8D9E0F1G", "username": "andre", "avatar_url": null },
          "role": "viewer",
          "permissions": "BASE_READ | POWER_ACTIONS",
          "joined_at": null,
          "invited_at": "2026-08-12T13:59:00Z",
          "last_invite_sent": "2026-08-12T13:59:00Z",
          "invite_resend_available_at": "2026-08-12T14:01:00Z",
          "pending": true,
          "is_owner": false
        }"#;

        let parsed: ServerMember = from_contract(json);
        assert_eq!(parsed.role, ServerRole::Viewer);
        assert_eq!(parsed.permissions, Permissions::from_role(ServerRole::Viewer));
        assert!(parsed.pending && parsed.joined_at.is_none());
        assert_round_trip(&parsed);
    }

    #[test]
    fn the_backup_the_contract_prints_reads_as_the_backup_we_declare() {
        let json = r#"{
          "id": "01JEXZ9K2QW8T7VN4M0P3RCB6D",
          "name": "Before the nether rebuild",
          "created_at": "2026-08-12T14:03:11Z",
          "status": "in_progress",
          "locked": false,
          "automated": false,
          "size_bytes": 0,
          "location": "local",
          "drive_state": null,
          "drive_web_link": null,
          "history": [
            {
              "operation_type": "create",
              "operation_id": "01JEXZ9K2QW8T7VN4M0P3RCB6E",
              "state": "ongoing",
              "scheduled_for": "2026-08-12T14:03:11Z",
              "started_at": "2026-08-12T14:03:12Z",
              "completed_at": null,
              "has_parent": false,
              "error": null,
              "should_prompt": true,
              "synthetic_legacy": false,
              "user_info": {
                "id": "01JZ8Q9V0RS2H5PT7YF3D1XKAE",
                "username": "max",
                "avatar_url": null
              }
            }
          ]
        }"#;

        assert_eq!(from_contract::<Backup>(json), backup());
    }

    #[test]
    fn a_state_the_contract_does_not_know_is_refused_on_both_sides() {
        assert!(serde_json::from_str::<ServerStatus>("\"suspended\"").is_err());
        assert!(serde_json::from_str::<OperationState>("\"error\"").is_err());
        assert!(serde_json::from_str::<PowerState>("\"installing\"").is_err());
        assert!(serde_json::from_str::<BackupStatus>("\"cancelled\"").is_err());
        assert!(serde_json::from_str::<LoaderId>("\"spigot\"").is_err());
        assert!(serde_json::from_str::<ServerRole>("\"support\"").is_err());

        assert!(ServerStatus::from_str("available").is_ok());
        assert_eq!(
            ServerStatus::from_str("suspended").unwrap_err().to_string(),
            "\"suspended\" is not a valid ServerStatus"
        );
    }

    async fn schema() -> sqlx::SqlitePool {
        let options = sqlx::sqlite::SqliteConnectOptions::new().in_memory(true).foreign_keys(true);
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .connect_with(options)
            .await
            .expect("an in-memory database");
        sqlx::migrate!("./migrations").run(&pool).await.expect("the migrations apply");
        pool
    }

    async fn a_user(pool: &sqlx::SqlitePool, id: Id) {
        sqlx::query(
            "INSERT INTO users (id, username, password_hash, role, created_at, updated_at)
             VALUES (?, ?, 'argon2', 'admin', ?, ?)",
        )
        .bind(id)
        .bind(id.to_string())
        .bind(Timestamp::now())
        .bind(Timestamp::now())
        .execute(pool)
        .await
        .expect("a panel user");
    }

    async fn a_server(pool: &sqlx::SqlitePool, id: Id, owner: Id) {
        sqlx::query(
            "INSERT INTO servers (id, name, owner_id, status, loader, memory_mib,
                                  created_at, updated_at)
             VALUES (?, 'Survival', ?, ?, ?, 4096, ?, ?)",
        )
        .bind(id)
        .bind(owner)
        .bind(ServerStatus::Available)
        .bind(LoaderId::Paper)
        .bind(Timestamp::now())
        .bind(Timestamp::now())
        .execute(pool)
        .await
        .expect("a server");
    }

    fn is_check_violation(err: sqlx::Error) -> bool {
        matches!(&err, sqlx::Error::Database(db) if {
            let text = db.message();
            text.contains("CHECK constraint failed")
                || text.contains("UNIQUE constraint failed")
                || text.contains("FOREIGN KEY constraint failed")
        })
    }

    #[tokio::test]
    async fn ids_and_timestamps_go_into_the_database_and_come_back_the_same() {
        let pool = schema().await;
        let owner = Id::new();
        let server = Id::new();
        a_user(&pool, owner).await;
        a_server(&pool, server, owner).await;

        let (read_id, status, loader, created): (Id, ServerStatus, Option<LoaderId>, Timestamp) =
            sqlx::query_as("SELECT id, status, loader, created_at FROM servers")
                .fetch_one(&pool)
                .await
                .expect("the row reads back");

        assert_eq!(read_id, server);
        assert_eq!(status, ServerStatus::Available);
        assert_eq!(loader, Some(LoaderId::Paper));
        assert_eq!(created.to_string().len(), "2026-08-12T14:03:11Z".len());

        let (id_text, status_text): (String, String) =
            sqlx::query_as("SELECT id, status FROM servers").fetch_one(&pool).await.unwrap();
        assert_eq!(id_text, server.to_string());
        assert_eq!(status_text, "available");
    }

    #[tokio::test]
    async fn a_state_outside_the_contract_does_not_reach_the_table() {
        let pool = schema().await;
        let owner = Id::new();
        let server = Id::new();
        a_user(&pool, owner).await;
        a_server(&pool, server, owner).await;

        let refused = [
            "UPDATE servers SET status = 'suspended'",
            "UPDATE servers SET loader = 'spigot'",
            "UPDATE servers SET name = ''",
            "UPDATE servers SET memory_mib = 256",
            "UPDATE users SET system_state = 'failed'",
            "UPDATE users SET cpu_mode = 'weight'",
            "INSERT INTO allocations (port, server_id, name, created_at)
             SELECT 80, id, 'Web', created_at FROM servers",
            "INSERT INTO server_members (id, server_id, user_id, role, invited_at)
             SELECT '01ARZ3NDEKTSV4RRFFQ69G5FAV', id, owner_id, 'owner', created_at FROM servers",
            "INSERT INTO operations (id, server_id, kind, state, progress, created_at)
             SELECT '01ARZ3NDEKTSV4RRFFQ69G5FAV', id, 'unarchive', 'ongoing', 42, created_at
             FROM servers",
            "INSERT INTO operations (id, server_id, kind, state, created_at)
             SELECT '01ARZ3NDEKTSV4RRFFQ69G5FAV', id, 'unarchive', 'error', created_at
             FROM servers",
            "INSERT INTO operations (id, server_id, kind, state, error_code, created_at)
             SELECT '01ARZ3NDEKTSV4RRFFQ69G5FAV', id, 'unarchive', 'failed', 'timeout', created_at
             FROM servers",
            "INSERT INTO audit_log (id, server_id, actor_user_id, action, created_at)
             SELECT '01ARZ3NDEKTSV4RRFFQ69G5FAV', id, owner_id, 'sftp_login', created_at
             FROM servers",
            "UPDATE panel_settings SET max_backups_per_server = 51",
            "UPDATE panel_settings SET port_pool_from = 30000, port_pool_to = 20000",
        ];

        for statement in refused {
            let err = sqlx::query(statement)
                .execute(&pool)
                .await
                .expect_err(&format!("the database accepted: {statement}"));
            assert!(is_check_violation(err), "wrong kind of refusal for: {statement}");
        }
    }

    #[tokio::test]
    async fn an_id_that_is_not_a_ulid_does_not_reach_the_table_either() {
        let pool = schema().await;
        let owner = Id::new();
        a_user(&pool, owner).await;

        let err = sqlx::query(
            "INSERT INTO servers (id, name, owner_id, status, memory_mib, created_at, updated_at)
             VALUES ('nope', 'Survival', ?, 'available', 4096, ?, ?)",
        )
        .bind(owner)
        .bind(Timestamp::now())
        .bind(Timestamp::now())
        .execute(&pool)
        .await
        .expect_err("a four letter id is not a ULID");
        assert!(is_check_violation(err));

        assert!("nope".parse::<Id>().is_err());
    }

    #[tokio::test]
    async fn one_backup_run_at_a_time_but_a_restore_may_bring_its_safety_copy() {
        let pool = schema().await;
        let owner = Id::new();
        let server = Id::new();
        a_user(&pool, owner).await;
        a_server(&pool, server, owner).await;

        let start = |kind: OperationKind, state: OperationState| {
            let pool = pool.clone();
            async move {
                sqlx::query(
                    "INSERT INTO operations (id, server_id, kind, state, created_at)
                     VALUES (?, ?, ?, ?, ?)",
                )
                .bind(Id::new())
                .bind(server)
                .bind(kind)
                .bind(state)
                .bind(Timestamp::now())
                .execute(&pool)
                .await
            }
        };

        start(OperationKind::BackupCreate, OperationState::Ongoing).await.expect("the first");
        start(OperationKind::BackupRestore, OperationState::Queued).await.expect("a restore");

        let err = start(OperationKind::BackupCreate, OperationState::Queued)
            .await
            .expect_err("a second open create is what 409 server_busy is for");
        assert!(is_check_violation(err));

        sqlx::query("UPDATE operations SET state = 'done' WHERE kind = 'backup_create'")
            .execute(&pool)
            .await
            .unwrap();
        start(OperationKind::BackupCreate, OperationState::Queued).await.expect("the next one");
    }

    #[tokio::test]
    async fn a_port_belongs_to_one_server_and_a_server_to_one_primary_port() {
        let pool = schema().await;
        let owner = Id::new();
        let first = Id::new();
        let second = Id::new();
        a_user(&pool, owner).await;
        a_server(&pool, first, owner).await;
        a_server(&pool, second, owner).await;

        let claim = |server: Id, port: u16, primary: bool| {
            let pool = pool.clone();
            async move {
                sqlx::query(
                    "INSERT INTO allocations (port, server_id, name, is_primary, created_at)
                     VALUES (?, ?, 'Minecraft', ?, ?)",
                )
                .bind(port)
                .bind(server)
                .bind(primary)
                .bind(Timestamp::now())
                .execute(&pool)
                .await
            }
        };

        claim(first, 25565, true).await.expect("the primary port");
        claim(first, 25566, false).await.expect("a second allocation");
        assert!(is_check_violation(claim(second, 25565, true).await.expect_err("port_in_use")));
        assert!(is_check_violation(claim(first, 25567, true).await.expect_err("already_primary")));
    }

    #[tokio::test]
    async fn deleting_a_user_who_owns_a_server_is_a_decision_and_not_a_side_effect() {
        let pool = schema().await;
        let owner = Id::new();
        let server = Id::new();
        a_user(&pool, owner).await;
        a_server(&pool, server, owner).await;
        sqlx::query("INSERT INTO backups (id, server_id, name, created_at) VALUES (?, ?, 'B', ?)")
            .bind(Id::new())
            .bind(server)
            .bind(Timestamp::now())
            .execute(&pool)
            .await
            .unwrap();

        let err = sqlx::query("DELETE FROM users WHERE id = ?")
            .bind(owner)
            .execute(&pool)
            .await
            .expect_err("the servers have to go first");
        assert!(is_check_violation(err));

        sqlx::query("DELETE FROM servers WHERE id = ?").bind(server).execute(&pool).await.unwrap();
        let (backups,): (i64,) =
            sqlx::query_as("SELECT count(*) FROM backups").fetch_one(&pool).await.unwrap();
        assert_eq!(backups, 0);
        sqlx::query("DELETE FROM users WHERE id = ?").bind(owner).execute(&pool).await.unwrap();
    }

    #[tokio::test]
    async fn the_panel_settings_are_one_row_with_the_defaults_the_contract_names() {
        let pool = schema().await;
        let (rows, backups, operations, upload, external): (i64, u32, u32, i64, bool) =
            sqlx::query_as(
                "SELECT count(*), max_backups_per_server, max_concurrent_operations,
                        max_upload_bytes, external_services_enabled
                 FROM panel_settings",
            )
            .fetch_one(&pool)
            .await
            .expect("a fresh database already has its settings");

        assert_eq!(rows, 1);
        assert_eq!(backups, 10);
        assert_eq!(operations, 2);
        assert_eq!(upload, 4 * 1024 * 1024 * 1024);
        assert!(external);

        let err = sqlx::query(
            "INSERT INTO panel_settings (id, port_pool_from, port_pool_to, default_memory_mib,
                 default_cpu_mode, default_cpu_cores, default_pids_max, max_upload_bytes,
                 max_backups_per_server, max_concurrent_operations, stop_grace_seconds, updated_at)
             VALUES (2, 25565, 25700, 4096, 'cap', 2.0, 512, 1024, 10, 2, 60, ?)",
        )
        .bind(Timestamp::now())
        .execute(&pool)
        .await
        .expect_err("there is one panel");
        assert!(is_check_violation(err));
    }

    #[tokio::test]
    async fn the_supervisor_key_survives_a_restart_of_the_panel() {
        let pool = schema().await;
        let owner = Id::new();
        let server = Id::new();
        a_user(&pool, owner).await;
        a_server(&pool, server, owner).await;

        let token = "0123456789abcdef0123456789abcdef";
        sqlx::query("UPDATE servers SET supervisor_token = ? WHERE id = ?")
            .bind(token)
            .bind(server)
            .execute(&pool)
            .await
            .unwrap();

        let known: Vec<(String, String)> =
            sqlx::query_as(
                "SELECT id, supervisor_token FROM servers WHERE supervisor_token IS NOT NULL",
            )
            .fetch_all(&pool)
            .await
                .expect("what Hub::load_tokens is fed at startup");
        assert_eq!(known, vec![(server.to_string(), token.to_owned())]);
    }

    #[test]
    fn paging_stays_inside_the_limits_the_contract_names() {
        let asked = OffsetPage { limit: Some(9000), offset: Some(200) };
        assert_eq!(asked.limit(200, 500), 500);
        assert_eq!(asked.offset(), 200);
        assert_eq!(OffsetPage::default().limit(200, 500), 200);
        assert_eq!(OffsetPage::default().offset(), 0);
        assert_eq!(OffsetPage { limit: Some(0), offset: None }.limit(50, 200), 1);
        assert_eq!(CursorPage::default().limit(100, 200), 100);

        let query: CursorPage =
            serde_json::from_str(r#"{"before":"01JZ8QK3F0V6WQ0X6M2N9CQ7RT"}"#).unwrap();
        assert_eq!(query.before, Some(id(OPERATION_ID)));
        assert_eq!(query.limit(50, 200), 50);
    }
}
