use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::project::ProjectName;

#[derive(Debug, Parser)]
#[command(
    name = "busy-nas",
    version,
    about = "Hand unfinished work between a NAS and one local checkout"
)]
pub struct Cli {
    /// Use another configuration file.
    #[arg(long, global = true, env = "BUSY_NAS_CONFIG")]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Claim a project and copy it from the NAS.
    Get { project: ProjectName },
    /// Snapshot and return a claimed checkout to the NAS.
    Put { project: ProjectName },
    /// Drop a claimed checkout without changing the NAS.
    Discard { project: ProjectName },
    /// Show projects and leases.
    Status,
    /// Replace a lease after its owner is unavailable.
    Reclaim {
        project: ProjectName,
        /// Confirm lease replacement.
        #[arg(long)]
        force: bool,
    },
}
