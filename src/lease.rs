use std::{fs, io::Write, path::Path};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{config::paths::AppPaths, project::ProjectName, BusyNasError, Result};

pub const FORMAT_VERSION: u8 = 1;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Lease {
    pub format_version: u8,
    pub project: String,
    pub token: String,
    pub created_at: DateTime<Utc>,
}

impl Lease {
    pub fn new(project: &ProjectName) -> Self {
        Self {
            format_version: FORMAT_VERSION,
            project: project.as_str().to_owned(),
            token: Uuid::new_v4().to_string(),
            created_at: Utc::now(),
        }
    }

    pub fn from_toml(raw: &str, project: &ProjectName) -> Result<Self> {
        let lease: Self = toml::from_str(raw)
            .map_err(|error| BusyNasError::InvalidLeaseMetadata(error.to_string()))?;
        lease.validate(project)?;
        Ok(lease)
    }

    pub fn to_toml(&self) -> Result<String> {
        Ok(toml::to_string(self)?)
    }

    fn validate(&self, project: &ProjectName) -> Result<()> {
        if self.format_version != FORMAT_VERSION {
            return Err(BusyNasError::InvalidLeaseMetadata(format!(
                "unsupported format version {}",
                self.format_version
            )));
        }
        if self.project != project.as_str() {
            return Err(BusyNasError::InvalidLeaseMetadata(format!(
                "project is `{}`, expected `{}`",
                self.project, project
            )));
        }
        if self.token.is_empty() {
            return Err(BusyNasError::InvalidLeaseMetadata(
                "token must not be empty".to_owned(),
            ));
        }
        Ok(())
    }
}

pub fn read(paths: &AppPaths, project: &ProjectName) -> Result<Lease> {
    let file = paths.lease_state_file(project);
    match fs::read_to_string(&file) {
        Ok(raw) => Lease::from_toml(&raw, project),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => read_legacy(paths, project),
        Err(error) => Err(BusyNasError::ConfigRead(error)),
    }
}

pub fn write(paths: &AppPaths, project: &ProjectName, lease: &Lease) -> Result<()> {
    let file = paths.lease_state_file(project);
    let parent = file.parent().expect("state file has a parent");
    fs::create_dir_all(parent)?;
    write_atomic(&file, lease.to_toml()?.as_bytes())
}

pub fn remove(paths: &AppPaths, project: &ProjectName) -> Result<()> {
    for file in [
        paths.lease_state_file(project),
        paths.legacy_lease_state_file(project),
    ] {
        match fs::remove_file(file) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(BusyNasError::ConfigRead(error)),
        }
    }
    Ok(())
}

#[derive(Deserialize)]
struct LegacyLease {
    project: String,
    token: String,
    created_at: DateTime<Utc>,
}

fn read_legacy(paths: &AppPaths, project: &ProjectName) -> Result<Lease> {
    let file = paths.legacy_lease_state_file(project);
    let raw = fs::read_to_string(&file).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            BusyNasError::LocalLeaseMissing(project.as_str().to_owned())
        } else {
            BusyNasError::ConfigRead(error)
        }
    })?;
    let lease: LegacyLease = toml::from_str(&raw)
        .map_err(|error| BusyNasError::InvalidLeaseMetadata(error.to_string()))?;
    if lease.project != project.as_str() || lease.token.is_empty() {
        return Err(BusyNasError::LocalLeaseMissing(project.as_str().to_owned()));
    }
    Ok(Lease {
        format_version: FORMAT_VERSION,
        project: lease.project,
        token: lease.token,
        created_at: lease.created_at,
    })
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

    use super::{read, remove, write, Lease, FORMAT_VERSION};
    use crate::{config::paths::AppPaths, project::ProjectName};

    #[test]
    fn lease_has_a_write_read_remove_lifecycle() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = AppPaths {
            config_file: PathBuf::from("unused.toml"),
            state_dir: temporary.path().join("state"),
        };
        let project = ProjectName::parse("demo").unwrap();
        let lease = Lease::new(&project);

        write(&paths, &project, &lease).unwrap();
        let stored = read(&paths, &project).unwrap();
        assert_eq!(stored.token, lease.token);
        assert_eq!(stored.format_version, FORMAT_VERSION);
        remove(&paths, &project).unwrap();
        assert!(read(&paths, &project).is_err());
    }

    #[test]
    fn lease_metadata_rejects_unknown_versions() {
        let project = ProjectName::parse("demo").unwrap();
        let error = Lease::from_toml(
            r#"
                format_version = 2
                project = "demo"
                token = "token"
                created_at = "2026-09-05T12:00:00Z"
            "#,
            &project,
        )
        .unwrap_err();
        assert!(error.to_string().contains("unsupported format version 2"));
    }

    #[test]
    fn legacy_local_state_remains_readable() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = AppPaths {
            config_file: PathBuf::from("unused.toml"),
            state_dir: temporary.path().join("state"),
        };
        let project = ProjectName::parse("demo").unwrap();
        let legacy_file = paths.legacy_lease_state_file(&project);
        std::fs::create_dir_all(legacy_file.parent().unwrap()).unwrap();
        std::fs::write(
            legacy_file,
            r#"
                project = "demo"
                token = "legacy-token"
                created_at = "2026-09-05T12:00:00Z"
            "#,
        )
        .unwrap();

        assert_eq!(read(&paths, &project).unwrap().token, "legacy-token");
    }
}
