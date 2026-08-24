use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::settings::runtimes::Search;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_bind")]
    pub bind: SocketAddr,
    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,
    #[serde(default = "default_helper_socket")]
    pub helper_socket: PathBuf,
    #[serde(skip)]
    pub java_search: Search,
}

fn default_bind() -> SocketAddr {
    "127.0.0.1:8080".parse().expect("valid default bind address")
}

fn default_data_dir() -> PathBuf {
    PathBuf::from("/var/lib/craftpanel")
}

fn default_helper_socket() -> PathBuf {
    PathBuf::from("/run/craftpanel/helper.sock")
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind: default_bind(),
            data_dir: default_data_dir(),
            helper_socket: default_helper_socket(),
            java_search: Search::default(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading config from {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing config at {}", path.display()))
    }

    pub fn database_path(&self) -> PathBuf {
        self.data_dir.join("panel.db")
    }

    pub fn users_dir(&self) -> PathBuf {
        self.data_dir.join("users")
    }

    pub fn cache_dir(&self) -> PathBuf {
        self.data_dir.join("cache")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_an_older_installer_wrote_still_opens() {
        let path = std::env::temp_dir()
            .join(format!("craftpanel-config-{}.toml", std::process::id()));
        std::fs::write(
            &path,
            "bind = \"0.0.0.0:9090\"\n\
             data_dir = \"/var/lib/craftpanel\"\n\
             helper_socket = \"/run/craftpanel/helper.sock\"\n\
             \n\
             [ports]\n\
             start = 25565\n\
             end = 25700\n",
        )
        .unwrap();

        let config = Config::load(&path);
        let _ = std::fs::remove_file(&path);
        let config = config.expect("a table the panel no longer knows is not an error");

        assert_eq!(config.bind.port(), 9090);
        assert_eq!(config.data_dir, PathBuf::from("/var/lib/craftpanel"));
    }
}
