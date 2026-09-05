use std::{io, path::PathBuf};

use thiserror::Error;

pub type Result<T> = std::result::Result<T, BusyNasError>;

#[derive(Debug, Error)]
pub enum BusyNasError {
    #[error("configuration file not found: {0}")]
    ConfigMissing(PathBuf),

    #[error("could not read configuration: {0}")]
    ConfigRead(#[from] io::Error),

    #[error("invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("could not parse configuration: {0}")]
    ConfigParse(#[from] toml::de::Error),

    #[error("could not serialize lease metadata: {0}")]
    LeaseSerialize(#[from] toml::ser::Error),

    #[error("invalid project name `{0}`; use a lowercase slug such as `my-project`")]
    InvalidProjectName(String),

    #[error("workspace root is unsafe: {0}")]
    UnsafeWorkspace(String),

    #[error("a local checkout already exists: {0}")]
    CheckoutExists(PathBuf),

    #[error("local checkout does not exist: {0}")]
    CheckoutMissing(PathBuf),

    #[error("no local lease state exists for project `{0}`")]
    LocalLeaseMissing(String),

    #[error("invalid lease metadata: {0}")]
    InvalidLeaseMetadata(String),

    #[error("the NAS lease for `{0}` is held by another machine or cannot be acquired")]
    LeaseHeld(String),

    #[error("the NAS lease for `{project}` does not match this machine's token")]
    LeaseMismatch { project: String },

    #[error("NAS source project does not exist: {0}")]
    RemoteProjectMissing(String),

    #[error("`reclaim` requires --force")]
    ForceRequired,

    #[error("failed to start `{program}`: {source}")]
    ProcessStart { program: String, source: io::Error },

    #[error("`{program}` failed with status {status}: {stderr}")]
    ProcessFailed {
        program: String,
        status: i32,
        stderr: String,
    },

    #[error("rsync verification found remaining differences: {0}")]
    TransferVerification(String),
}
