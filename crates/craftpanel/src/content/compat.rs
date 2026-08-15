use std::collections::BTreeSet;

use crate::model::{ContentProjectType, LoaderId};

use super::modrinth::MrVersion;

const ALIAS_GROUPS: [&[&str]; 2] = [&["paper", "purpur", "spigot", "bukkit"], &["neoforge", "neo"]];

const LOADERLESS: [&str; 4] = ["shader", "shaderpack", "resourcepack", "datapack"];

pub fn normalise_loader(loader: &str) -> String {
    loader.to_lowercase().replace(['_', '-', ' '], "")
}

pub fn aliases(loader: &str) -> BTreeSet<String> {
    let loader = normalise_loader(loader);
    let mut names = BTreeSet::from([loader.clone()]);
    if let Some(group) = ALIAS_GROUPS.iter().find(|group| group.contains(&loader.as_str())) {
        names.extend(group.iter().map(|name| (*name).to_owned()));
    }
    names
}

#[derive(Debug, Clone)]
pub struct Target {
    pub game_version: String,
    pub loader: LoaderId,
    pub project_type: Option<ContentProjectType>,
}

impl Target {
    pub fn new(game_version: impl Into<String>, loader: LoaderId) -> Self {
        Self { game_version: game_version.into(), loader, project_type: None }
    }

    pub fn of(mut self, project_type: Option<ContentProjectType>) -> Self {
        self.project_type = project_type;
        self
    }
}

pub fn matches(version: &MrVersion, target: &Target) -> bool {
    if target.game_version.is_empty() || !version.game_versions.contains(&target.game_version) {
        return false;
    }

    let loaders: Vec<String> = version.loaders.iter().map(|name| normalise_loader(name)).collect();
    match target.project_type {
        Some(ContentProjectType::Datapack) => return loaders.iter().any(|name| name == "datapack"),
        Some(kind) if LOADERLESS.contains(&kind.as_str()) => return true,
        _ => {}
    }

    let wanted = aliases(target.loader.as_str());
    loaders.iter().any(|loader| wanted.contains(loader))
}

pub fn matches_modpack(version: &MrVersion, game_version: &str) -> bool {
    if !version.game_versions.iter().any(|known| known == game_version) {
        return false;
    }
    let loaders: Vec<String> = version.loaders.iter().map(|name| normalise_loader(name)).collect();
    loaders.is_empty() || loaders.iter().all(|loader| loader == "mrpack")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::modrinth::a_version;

    fn version(loaders: &[&str], games: &[&str]) -> MrVersion {
        let mut version = a_version("v", "P", "release", "2026-01-01T00:00:00Z");
        version.loaders = loaders.iter().map(|name| (*name).to_owned()).collect();
        version.game_versions = games.iter().map(|name| (*name).to_owned()).collect();
        version
    }

    #[test]
    fn a_purpur_server_takes_a_plugin_that_only_names_paper() {
        let plugin = version(&["paper"], &["1.21.1"]);
        assert!(matches(&plugin, &Target::new("1.21.1", LoaderId::Purpur)));
        assert!(matches(&plugin, &Target::new("1.21.1", LoaderId::Paper)));
        assert!(!matches(&plugin, &Target::new("1.21.1", LoaderId::Fabric)));
    }

    #[test]
    fn neo_and_neoforge_are_the_same_loader_under_two_names() {
        let mod_file = version(&["neo"], &["1.21.1"]);
        assert!(matches(&mod_file, &Target::new("1.21.1", LoaderId::NeoForge)));
        assert!(!matches(&mod_file, &Target::new("1.21.1", LoaderId::Forge)));
    }

    #[test]
    fn the_game_version_is_never_waved_through() {
        let mod_file = version(&["fabric"], &["1.20.1"]);
        assert!(!matches(&mod_file, &Target::new("1.21.1", LoaderId::Fabric)));
        assert!(matches(&mod_file, &Target::new("1.20.1", LoaderId::Fabric)));
    }

    #[test]
    fn a_datapack_is_matched_on_the_word_datapack_and_nothing_else() {
        let pack = version(&["datapack"], &["1.21.1"]);
        let target = Target::new("1.21.1", LoaderId::Vanilla).of(Some(ContentProjectType::Datapack));
        assert!(matches(&pack, &target));

        let jar = version(&["fabric"], &["1.21.1"]);
        assert!(!matches(&jar, &target));
    }

    #[test]
    fn a_shader_needs_no_loader_at_all() {
        let shader = version(&[], &["1.21.1"]);
        let target = Target::new("1.21.1", LoaderId::Paper).of(Some(ContentProjectType::Shader));
        assert!(matches(&shader, &target));
    }

    #[test]
    fn a_modpack_carries_either_no_loader_or_the_word_mrpack() {
        assert!(matches_modpack(&version(&[], &["1.21.1"]), "1.21.1"));
        assert!(matches_modpack(&version(&["mrpack"], &["1.21.1"]), "1.21.1"));
        assert!(!matches_modpack(&version(&["fabric"], &["1.21.1"]), "1.21.1"));
        assert!(!matches_modpack(&version(&[], &["1.20.1"]), "1.21.1"));
    }

    #[test]
    fn spelling_a_loader_with_a_dash_or_an_underscore_is_the_same_loader() {
        assert_eq!(normalise_loader("Neo-Forge"), "neoforge");
        assert_eq!(normalise_loader("quilt_loader"), "quiltloader");
        assert!(aliases("PAPER").contains("purpur"));
    }
}
