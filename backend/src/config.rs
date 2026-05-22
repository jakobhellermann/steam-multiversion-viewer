//! On-disk user configuration.

use camino::{Utf8Path, Utf8PathBuf};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub store_root: Utf8PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            store_root: default_store_root(),
        }
    }
}

impl Config {
    pub fn load_or_default() -> Result<Self, ConfigError> {
        let path = config_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(&path)?;
        let cfg = serde_json::from_str(&raw)?;
        Ok(cfg)
    }

    pub fn save(&self) -> Result<(), ConfigError> {
        let path = config_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let raw = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, raw)?;
        Ok(())
    }
}

pub fn config_path() -> Result<Utf8PathBuf, ConfigError> {
    let dirs =
        ProjectDirs::from("", "", "steam-multiversion-viewer").ok_or(ConfigError::NoProjectDirs)?;
    let path = Utf8Path::from_path(dirs.config_dir())
        .ok_or_else(|| ConfigError::NonUtf8Path(dirs.config_dir().display().to_string()))?
        .to_path_buf();
    Ok(path.join("config.json"))
}

fn default_store_root() -> Utf8PathBuf {
    ProjectDirs::from("", "", "steam-multiversion-viewer")
        .and_then(|d| Utf8Path::from_path(d.data_dir()).map(|p| p.to_path_buf()))
        .map(|p| p.join("store"))
        .unwrap_or_else(|| Utf8PathBuf::from("store"))
}

#[derive(Debug)]
pub enum ConfigError {
    Io(std::io::Error),
    Json(serde_json::Error),
    NoProjectDirs,
    NonUtf8Path(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io error: {e}"),
            Self::Json(e) => write!(f, "json error: {e}"),
            Self::NoProjectDirs => f.write_str("project directories unavailable on this platform"),
            Self::NonUtf8Path(p) => write!(f, "non-utf8 path: {p}"),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Json(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for ConfigError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<serde_json::Error> for ConfigError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}
