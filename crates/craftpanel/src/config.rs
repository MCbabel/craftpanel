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
    #[serde(default)]
    pub ports: PortPool,
    #[serde(skip)]
    pub java_search: Search,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortPool {
    pub start: u16,
    pub end: u16,
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

impl Default for PortPool {
    fn default() -> Self {
        Self { start: 25565, end: 25700 }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind: default_bind(),
            data_dir: default_data_dir(),
            helper_socket: default_helper_socket(),
            ports: PortPool::default(),
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
