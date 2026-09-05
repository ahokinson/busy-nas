use std::{fs, io::Write, path::Path};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{paths::AppPaths, project::ProjectName, BusyNasError, Result};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LeaseState {
    pub project: String,
    pub token: String,
    pub created_at: DateTime<Utc>,
}

impl LeaseState {
    pub fn new(project: &ProjectName) -> Self {
        Self {
            project: project.as_str().to_owned(),
            token: Uuid::new_v4().to_string(),
            created_at: Utc::now(),
        }
    }
}

pub fn read(paths: &AppPaths, project: &ProjectName) -> Result<LeaseState> {
    let file = paths.lease_state_file(project);
    let raw = fs::read_to_string(&file).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            BusyNasError::LocalLeaseMissing(project.as_str().to_owned())
        } else {
            BusyNasError::ConfigRead(error)
        }
    })?;
    let state: LeaseState = toml::from_str(&raw)?;
    if state.project != project.as_str() {
        return Err(BusyNasError::LocalLeaseMissing(project.as_str().to_owned()));
    }
    Ok(state)
}

pub fn write(paths: &AppPaths, project: &ProjectName, state: &LeaseState) -> Result<()> {
    let file = paths.lease_state_file(project);
    let parent = file.parent().expect("state file has a parent");
    fs::create_dir_all(parent)?;
    write_atomic(&file, toml::to_string(state)?.as_bytes())
}

pub fn remove(paths: &AppPaths, project: &ProjectName) -> Result<()> {
    let file = paths.lease_state_file(project);
    match fs::remove_file(file) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(BusyNasError::ConfigRead(error)),
    }
}

fn write_atomic(file: &Path, contents: &[u8]) -> Result<()> {
    let temporary = file.with_extension(format!("{}.tmp", Uuid::new_v4()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut handle = options.open(&temporary)?;
    handle.write_all(contents)?;
    handle.sync_all()?;
    fs::rename(temporary, file)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{read, remove, write, LeaseState};
    use crate::{paths::AppPaths, project::ProjectName};

    #[test]
    fn lease_state_has_a_write_read_remove_lifecycle() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = AppPaths {
            config_file: PathBuf::from("unused.toml"),
            state_dir: temporary.path().join("state"),
        };
        let project = ProjectName::parse("demo").unwrap();
        let state = LeaseState::new(&project);

        write(&paths, &project, &state).unwrap();
        assert_eq!(read(&paths, &project).unwrap().token, state.token);
        remove(&paths, &project).unwrap();
        assert!(read(&paths, &project).is_err());
    }
}
