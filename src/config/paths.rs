use std::path::{Path, PathBuf};

use directories::ProjectDirs;

use crate::project::ProjectName;

#[derive(Clone, Debug)]
pub struct AppPaths {
    pub config_file: PathBuf,
    pub state_dir: PathBuf,
}

impl AppPaths {
    pub fn discover(config_file: Option<PathBuf>) -> Self {
        let dirs = ProjectDirs::from("dev", "busy-nas", "busy-nas")
            .expect("a platform state directory must exist");
        Self {
            config_file: config_file.unwrap_or_else(|| dirs.config_dir().join("config.toml")),
            state_dir: dirs
                .state_dir()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| dirs.data_local_dir().join("state")),
        }
    }

    pub fn lease_state_file(&self, project: &ProjectName) -> PathBuf {
        self.lease_state_dir()
            .join(format!("{}.toml", project.as_str()))
    }

    pub fn lease_state_dir(&self) -> PathBuf {
        self.state_dir.join("leases")
    }

    pub fn legacy_lease_state_file(&self, project: &ProjectName) -> PathBuf {
        self.legacy_lease_state_dir()
            .join(format!("{}.toml", project.as_str()))
    }

    pub fn legacy_lease_state_dir(&self) -> PathBuf {
        self.state_dir.join("checkouts")
    }
}
