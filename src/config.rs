use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use directories::ProjectDirs;
use serde::Deserialize;

use crate::{BusyNasError, Result};

pub mod paths;

const DEFAULT_REMOTE_ROOT: &str = "/srv/developer";
const DEFAULT_RETENTION: usize = 20;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub nas: NasConfig,
    #[serde(default = "default_workspace_root")]
    pub workspace_root: PathBuf,
    #[serde(default = "default_snapshot_retention")]
    pub snapshot_retention: usize,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NasConfig {
    pub host: String,
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default = "default_remote_root")]
    pub root: PathBuf,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                BusyNasError::ConfigMissing(path.to_owned())
            } else {
                BusyNasError::ConfigRead(error)
            }
        })?;
        Self::parse(&raw)
    }

    pub fn parse(raw: &str) -> Result<Self> {
        let config: Self = toml::from_str(raw)?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if !is_safe_host_component(&self.nas.host) {
            return Err(BusyNasError::InvalidConfig(
                "nas.host may contain only letters, digits, dots, or hyphens".into(),
            ));
        }
        if let Some(user) = &self.nas.user {
            if !is_safe_host_component(user) {
                return Err(BusyNasError::InvalidConfig(
                    "nas.user may contain only letters, digits, dots, or hyphens".into(),
                ));
            }
        }
        validate_remote_root(&self.nas.root)?;
        if !self.workspace_root.is_absolute() {
            return Err(BusyNasError::InvalidConfig(
                "workspace_root must be an absolute local path".into(),
            ));
        }
        if self.workspace_root.to_string_lossy().contains(':') {
            return Err(BusyNasError::InvalidConfig(
                "workspace_root may not contain ':' because rsync would treat it as a remote endpoint"
                    .into(),
            ));
        }
        if self.snapshot_retention == 0 {
            return Err(BusyNasError::InvalidConfig(
                "snapshot_retention must be at least 1".into(),
            ));
        }
        Ok(())
    }

    pub fn remote_target(&self) -> String {
        match &self.nas.user {
            Some(user) => format!("{user}@{}", self.nas.host),
            None => self.nas.host.clone(),
        }
    }
}

pub fn default_config_file() -> PathBuf {
    ProjectDirs::from("dev", "busy-nas", "busy-nas")
        .expect("a platform configuration directory must exist")
        .config_dir()
        .join("config.toml")
}

fn default_workspace_root() -> PathBuf {
    directories::BaseDirs::new()
        .expect("a home directory must exist")
        .home_dir()
        .join("Developer")
}

fn default_remote_root() -> PathBuf {
    PathBuf::from(DEFAULT_REMOTE_ROOT)
}

fn default_snapshot_retention() -> usize {
    DEFAULT_RETENTION
}

fn is_safe_host_component(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
}

fn validate_remote_root(root: &Path) -> Result<()> {
    if !root.is_absolute()
        || root.as_os_str().is_empty()
        || root
            .components()
            .any(|component| !matches!(component, Component::RootDir | Component::Normal(_)))
        || root.to_string_lossy().bytes().any(|byte| {
            !(byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'_' | b'-' | b'.'))
        })
    {
        return Err(BusyNasError::InvalidConfig(
            "nas.root must be an absolute, shell-safe path without traversal components".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::Config;

    #[test]
    fn parses_user_configuration_and_defaults() {
        let config = Config::parse(
            r#"
                workspace_root = "/Users/ada/Developer"

                [nas]
                host = "nas.home"
                user = "developer"
            "#,
        )
        .unwrap();

        assert_eq!(config.nas.root.to_string_lossy(), "/srv/developer");
        assert_eq!(config.snapshot_retention, 20);
        assert_eq!(config.remote_target(), "developer@nas.home");
    }

    #[test]
    fn rejects_unsafe_remote_root() {
        let error = Config::parse(
            r#"
                workspace_root = "/tmp/workspace"
                [nas]
                host = "nas"
                root = "/srv/developer/../other"
            "#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("nas.root"));
    }
}
