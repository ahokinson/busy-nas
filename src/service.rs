use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

use chrono::Utc;

use crate::{
    config::Config,
    filters::rsync_filter_args,
    paths::AppPaths,
    process::{CommandOutput, CommandRunner, CommandSpec, ProcessRunner},
    project::ProjectName,
    state::{self, LeaseState},
    workspace::validate_local_workspace,
    BusyNasError, Result,
};

#[derive(Clone, Debug)]
pub struct ProgramPaths {
    pub ssh: PathBuf,
    pub rsync: PathBuf,
}

impl ProgramPaths {
    pub fn system() -> Self {
        Self {
            ssh: PathBuf::from("ssh"),
            rsync: PathBuf::from("rsync"),
        }
    }

    pub fn from_process_runner(runner: &ProcessRunner) -> Self {
        Self {
            ssh: runner.ssh_program.clone(),
            rsync: runner.rsync_program.clone(),
        }
    }
}

#[derive(Debug, Default, Eq, PartialEq)]
pub struct Status {
    pub projects: Vec<String>,
    pub nas_leases: Vec<String>,
    pub local_leases: Vec<String>,
}

impl Status {
    pub fn render(&self) -> String {
        fn section(name: &str, entries: &[String], output: &mut String) {
            output.push_str(name);
            output.push_str(":\n");
            if entries.is_empty() {
                output.push_str("  (none)\n");
            } else {
                for entry in entries {
                    output.push_str("  ");
                    output.push_str(entry);
                    output.push('\n');
                }
            }
        }

        let mut output = String::new();
        section("NAS projects", &self.projects, &mut output);
        section("Active NAS leases", &self.nas_leases, &mut output);
        section("Local active checkouts", &self.local_leases, &mut output);
        output
    }
}

pub struct Service<'a, R> {
    config: &'a Config,
    paths: &'a AppPaths,
    runner: &'a mut R,
    programs: ProgramPaths,
}

impl<'a, R: CommandRunner> Service<'a, R> {
    pub fn new(
        config: &'a Config,
        paths: &'a AppPaths,
        runner: &'a mut R,
        programs: ProgramPaths,
    ) -> Self {
        Self {
            config,
            paths,
            runner,
            programs,
        }
    }

    pub fn get(&mut self, project: &ProjectName) -> Result<()> {
        self.require_safe_workspace()?;
        let checkout = self.checkout_path(project);
        if checkout.exists() {
            return Err(BusyNasError::CheckoutExists(checkout));
        }

        let acquisition = self.acquire_or_resume_lease(project)?;
        let transfer = (|| {
            if !self.remote_project_exists(project)? {
                return Err(BusyNasError::RemoteProjectMissing(
                    project.as_str().to_owned(),
                ));
            }
            self.rsync_from_nas(project, &checkout)
        })();

        if let Err(error) = transfer {
            // `get` created this destination.
            remove_checkout_if_present(&checkout)?;
            if acquisition.newly_acquired {
                self.release_lease_if_matching(project, &acquisition.state.token)?;
                state::remove(self.paths, project)?;
            }
            return Err(error);
        }
        Ok(())
    }

    pub fn put(&mut self, project: &ProjectName) -> Result<()> {
        self.require_safe_workspace()?;
        let checkout = self.checkout_path(project);
        if !checkout.is_dir() {
            return Err(BusyNasError::CheckoutMissing(checkout));
        }
        let lease = state::read(self.paths, project)?;
        self.require_matching_lease(project, &lease.token)?;
        if !self.remote_project_exists(project)? {
            return Err(BusyNasError::RemoteProjectMissing(
                project.as_str().to_owned(),
            ));
        }

        self.create_snapshot(project)?;
        self.rsync_to_nas(&checkout, project)?;
        self.verify_to_nas(&checkout, project)?;
        self.prune_snapshots(project)?;

        // Do not release the lease if local cleanup fails.
        fs::remove_dir_all(&checkout)?;
        self.release_lease_if_matching(project, &lease.token)?;
        state::remove(self.paths, project)?;
        Ok(())
    }

    pub fn discard(&mut self, project: &ProjectName) -> Result<()> {
        self.require_safe_workspace()?;
        let checkout = self.checkout_path(project);
        if checkout.exists() && !checkout.is_dir() {
            return Err(BusyNasError::CheckoutMissing(checkout));
        }
        let lease = state::read(self.paths, project)?;
        self.require_matching_lease(project, &lease.token)?;

        // Allow retries after local cleanup succeeded but lease release failed.
        if checkout.is_dir() {
            fs::remove_dir_all(&checkout)?;
        }
        self.release_lease_if_matching(project, &lease.token)?;
        state::remove(self.paths, project)
    }

    pub fn reclaim(&mut self, project: &ProjectName, force: bool) -> Result<()> {
        if !force {
            return Err(BusyNasError::ForceRequired);
        }
        self.require_safe_workspace()?;
        let checkout = self.checkout_path(project);
        if checkout.exists() {
            return Err(BusyNasError::CheckoutExists(checkout));
        }

        // Reclaim creates a new local token; `get` will resume it.
        let lease_path = self.lease_path(project);
        self.remote().remove_dir_all(&lease_path)?;
        state::remove(self.paths, project)?;
        let acquisition = self.acquire_or_resume_lease(project)?;
        debug_assert!(acquisition.newly_acquired);
        Ok(())
    }

    pub fn status(&mut self) -> Result<Status> {
        let remote_root = self.config.nas.root.clone();
        let mut projects = self.remote().list_directories(&remote_root)?;
        projects.retain(|name| name != ".busy-nas");
        projects.sort();

        let leases_root = self.leases_root();
        let mut nas_leases = self.remote().list_directories(&leases_root)?;
        nas_leases.sort();

        let mut local_leases = Vec::new();
        let local_directory = self.paths.state_dir.join("checkouts");
        if local_directory.is_dir() {
            for entry in fs::read_dir(local_directory)? {
                let entry = entry?;
                if entry.file_type()?.is_file() {
                    if let Some(name) = entry.path().file_stem().and_then(|name| name.to_str()) {
                        local_leases.push(name.to_owned());
                    }
                }
            }
            local_leases.sort();
        }

        Ok(Status {
            projects,
            nas_leases,
            local_leases,
        })
    }

    fn require_safe_workspace(&self) -> Result<()> {
        validate_local_workspace(&self.config.workspace_root)
    }

    fn checkout_path(&self, project: &ProjectName) -> PathBuf {
        self.config.workspace_root.join(project.as_str())
    }

    fn remote_project_path(&self, project: &ProjectName) -> PathBuf {
        self.config.nas.root.join(project.as_str())
    }

    fn control_root(&self) -> PathBuf {
        self.config.nas.root.join(".busy-nas")
    }

    fn leases_root(&self) -> PathBuf {
        self.control_root().join("leases")
    }

    fn lease_path(&self, project: &ProjectName) -> PathBuf {
        self.leases_root().join(project.as_str())
    }

    fn lease_token_path(&self, project: &ProjectName) -> PathBuf {
        self.lease_path(project).join("token")
    }

    fn snapshots_root(&self, project: &ProjectName) -> PathBuf {
        self.control_root().join("snapshots").join(project.as_str())
    }

    fn remote(&mut self) -> Remote<'_, R> {
        Remote {
            target: self.config.remote_target(),
            runner: self.runner,
            programs: &self.programs,
        }
    }

    fn acquire_or_resume_lease(&mut self, project: &ProjectName) -> Result<Acquisition> {
        match state::read(self.paths, project) {
            Ok(existing) => {
                self.require_matching_lease(project, &existing.token)?;
                Ok(Acquisition {
                    state: existing,
                    newly_acquired: false,
                })
            }
            Err(BusyNasError::LocalLeaseMissing(_)) => self.acquire_new_lease(project),
            Err(error) => Err(error),
        }
    }

    fn acquire_new_lease(&mut self, project: &ProjectName) -> Result<Acquisition> {
        let control_root = self.control_root();
        let lease_path = self.lease_path(project);
        let token_path = self.lease_token_path(project);
        {
            let mut remote = self.remote();
            remote.ensure_directory(&control_root)?;
            remote.ensure_directory(&control_root.join("leases"))?;
            remote.ensure_directory(&control_root.join("snapshots"))?;
            if !remote.make_directory(&lease_path)? {
                return Err(BusyNasError::LeaseHeld(project.as_str().to_owned()));
            }
        }

        let lease = LeaseState::new(project);
        let write_result = self
            .remote()
            .install_file(&token_path, format!("{}\n", lease.token).into_bytes());
        if let Err(error) = write_result {
            self.remote().remove_dir_all(&lease_path)?;
            return Err(error);
        }
        if let Err(error) = state::write(self.paths, project, &lease) {
            self.remote().remove_dir_all(&lease_path)?;
            return Err(error);
        }
        Ok(Acquisition {
            state: lease,
            newly_acquired: true,
        })
    }

    fn require_matching_lease(
        &mut self,
        project: &ProjectName,
        expected_token: &str,
    ) -> Result<()> {
        let token_path = self.lease_token_path(project);
        let output = self.remote().run("cat", [token_path.into_os_string()])?;
        if !output.success || output.stdout_text().trim() != expected_token {
            return Err(BusyNasError::LeaseMismatch {
                project: project.as_str().to_owned(),
            });
        }
        Ok(())
    }

    fn release_lease_if_matching(&mut self, project: &ProjectName, token: &str) -> Result<()> {
        self.require_matching_lease(project, token)?;
        let lease_path = self.lease_path(project);
        self.remote().remove_dir_all(&lease_path)
    }

    fn remote_project_exists(&mut self, project: &ProjectName) -> Result<bool> {
        let remote_path = self.remote_project_path(project);
        self.remote().is_directory(&remote_path)
    }

    fn rsync_from_nas(&mut self, project: &ProjectName, checkout: &Path) -> Result<()> {
        let source = self.remote_endpoint(&self.remote_project_path(project), true);
        let destination = checkout.as_os_str().to_owned();
        let spec = self.rsync_spec(source, destination, false, false);
        self.run_checked(&spec)?;
        Ok(())
    }

    fn rsync_to_nas(&mut self, checkout: &Path, project: &ProjectName) -> Result<()> {
        let source = with_trailing_slash(checkout);
        let destination = self.remote_endpoint(&self.remote_project_path(project), true);
        let spec = self.rsync_spec(source, destination, true, false);
        self.run_checked(&spec)?;
        Ok(())
    }

    fn verify_to_nas(&mut self, checkout: &Path, project: &ProjectName) -> Result<()> {
        let source = with_trailing_slash(checkout);
        let destination = self.remote_endpoint(&self.remote_project_path(project), true);
        let spec = self.rsync_spec(source, destination, true, true);
        let output = self.run_checked(&spec)?;
        let differences = output.stdout_text();
        if differences.trim().is_empty() {
            Ok(())
        } else {
            Err(BusyNasError::TransferVerification(differences))
        }
    }

    fn create_snapshot(&mut self, project: &ProjectName) -> Result<()> {
        let root = self.snapshots_root(project);
        let snapshot_name = format!(
            "{}-{}",
            Utc::now().format("%Y%m%dT%H%M%S%.3fZ"),
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        );
        let snapshot = root.join(snapshot_name);
        let source = self.remote_project_path(project);
        let mut remote = self.remote();
        remote.ensure_directory(&root)?;
        remote.ensure_directory(&snapshot)?;
        remote.rsync_local_to_local(&source, &snapshot)
    }

    fn prune_snapshots(&mut self, project: &ProjectName) -> Result<()> {
        let root = self.snapshots_root(project);
        let snapshots = self.remote().list_directories(&root)?;
        let excess = snapshots
            .len()
            .saturating_sub(self.config.snapshot_retention);
        for snapshot in snapshots.into_iter().take(excess) {
            self.remote().remove_dir_all(&root.join(snapshot))?;
        }
        Ok(())
    }

    fn remote_endpoint(&self, path: &Path, trailing_slash: bool) -> OsString {
        let mut output = format!("{}:{}", self.config.remote_target(), path.display());
        if trailing_slash {
            output.push('/');
        }
        output.into()
    }

    fn rsync_spec(
        &self,
        source: OsString,
        destination: OsString,
        delete: bool,
        dry_run: bool,
    ) -> CommandSpec {
        let mut args: Vec<OsString> = vec!["-a".into(), "--protect-args".into()];
        args.extend(rsync_filter_args().into_iter().map(OsString::from));
        if delete {
            args.push("--delete".into());
        }
        if dry_run {
            args.push("--dry-run".into());
            args.push("--itemize-changes".into());
        }
        args.push("-e".into());
        args.push(self.programs.ssh.clone().into_os_string());
        args.push(source);
        args.push(destination);
        CommandSpec {
            program: self.programs.rsync.clone(),
            args,
            stdin: None,
        }
    }

    fn run_checked(&mut self, spec: &CommandSpec) -> Result<CommandOutput> {
        let output = self.runner.run(spec)?;
        if output.success {
            Ok(output)
        } else {
            Err(BusyNasError::ProcessFailed {
                program: spec.program.display().to_string(),
                status: output.status,
                stderr: output.stderr_text(),
            })
        }
    }
}

struct Acquisition {
    state: LeaseState,
    newly_acquired: bool,
}

struct Remote<'a, R> {
    target: String,
    runner: &'a mut R,
    programs: &'a ProgramPaths,
}

impl<R: CommandRunner> Remote<'_, R> {
    fn run(
        &mut self,
        command: &str,
        command_args: impl IntoIterator<Item = OsString>,
    ) -> Result<CommandOutput> {
        let mut args = vec![
            OsString::from("--"),
            OsString::from(&self.target),
            OsString::from(command),
        ];
        args.extend(command_args);
        self.runner.run(&CommandSpec {
            program: self.programs.ssh.clone(),
            args,
            stdin: None,
        })
    }

    fn run_checked(
        &mut self,
        command: &str,
        args: impl IntoIterator<Item = OsString>,
    ) -> Result<CommandOutput> {
        let spec_args: Vec<OsString> = args.into_iter().collect();
        let output = self.run(command, spec_args)?;
        if output.success {
            Ok(output)
        } else {
            Err(BusyNasError::ProcessFailed {
                program: format!("{} {command}", self.programs.ssh.display()),
                status: output.status,
                stderr: output.stderr_text(),
            })
        }
    }

    fn ensure_directory(&mut self, path: &Path) -> Result<()> {
        self.run_checked("mkdir", [OsString::from("-p"), path.as_os_str().to_owned()])?;
        Ok(())
    }

    fn make_directory(&mut self, path: &Path) -> Result<bool> {
        Ok(self.run("mkdir", [path.as_os_str().to_owned()])?.success)
    }

    fn remove_dir_all(&mut self, path: &Path) -> Result<()> {
        self.run_checked("rm", [OsString::from("-rf"), path.as_os_str().to_owned()])?;
        Ok(())
    }

    fn install_file(&mut self, path: &Path, contents: Vec<u8>) -> Result<()> {
        let mut args = vec![
            OsString::from("-m"),
            OsString::from("600"),
            OsString::from("/dev/stdin"),
            path.as_os_str().to_owned(),
        ];
        let mut ssh_args = vec![
            OsString::from("--"),
            OsString::from(&self.target),
            OsString::from("install"),
        ];
        ssh_args.append(&mut args);
        let spec = CommandSpec {
            program: self.programs.ssh.clone(),
            args: ssh_args,
            stdin: Some(contents),
        };
        let output = self.runner.run(&spec)?;
        if output.success {
            Ok(())
        } else {
            Err(BusyNasError::ProcessFailed {
                program: format!("{} install", self.programs.ssh.display()),
                status: output.status,
                stderr: output.stderr_text(),
            })
        }
    }

    fn is_directory(&mut self, path: &Path) -> Result<bool> {
        Ok(self
            .run("test", [OsString::from("-d"), path.as_os_str().to_owned()])?
            .success)
    }

    fn list_directories(&mut self, path: &Path) -> Result<Vec<String>> {
        if !self.is_directory(path)? {
            return Ok(Vec::new());
        }
        let output = self.run_checked(
            "find",
            [
                path.as_os_str().to_owned(),
                OsString::from("-mindepth"),
                OsString::from("1"),
                OsString::from("-maxdepth"),
                OsString::from("1"),
                OsString::from("-type"),
                OsString::from("d"),
                OsString::from("-printf"),
                OsString::from("%f\\n"),
            ],
        )?;
        let mut directories = output
            .stdout_text()
            .lines()
            .filter(|line| !line.is_empty())
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        directories.sort();
        Ok(directories)
    }

    fn rsync_local_to_local(&mut self, source: &Path, destination: &Path) -> Result<()> {
        self.run_checked(
            "rsync",
            [
                OsString::from("-a"),
                with_trailing_slash(source),
                with_trailing_slash(destination),
            ],
        )?;
        Ok(())
    }
}

fn with_trailing_slash(path: &Path) -> OsString {
    let mut value = path.as_os_str().to_os_string();
    value.push("/");
    value
}

fn remove_checkout_if_present(path: &Path) -> Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(BusyNasError::ConfigRead(error)),
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, ffi::OsString, path::PathBuf};

    use super::{ProgramPaths, Service};
    use crate::{
        config::Config,
        paths::AppPaths,
        process::{CommandOutput, CommandRunner, CommandSpec},
    };

    struct RecordingRunner {
        results: VecDeque<CommandOutput>,
        specs: Vec<CommandSpec>,
    }

    impl RecordingRunner {
        fn successful() -> Self {
            Self {
                results: std::iter::repeat_with(|| CommandOutput {
                    success: true,
                    status: 0,
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                })
                .take(32)
                .collect(),
                specs: Vec::new(),
            }
        }
    }

    impl CommandRunner for RecordingRunner {
        fn run(&mut self, spec: &CommandSpec) -> crate::Result<CommandOutput> {
            self.specs.push(spec.clone());
            Ok(self
                .results
                .pop_front()
                .expect("test supplied enough process results"))
        }
    }

    fn config() -> Config {
        Config::parse(
            r#"
                workspace_root = "/tmp/busy-nas-workspace"
                snapshot_retention = 20
                [nas]
                host = "nas.example"
                root = "/srv/developer"
            "#,
        )
        .unwrap()
    }

    #[test]
    fn upload_rsync_uses_delete_filters_and_separate_arguments() {
        let config = config();
        let paths = AppPaths {
            config_file: PathBuf::from("/tmp/config.toml"),
            state_dir: PathBuf::from("/tmp/state"),
        };
        let mut runner = RecordingRunner::successful();
        let service = Service::new(&config, &paths, &mut runner, ProgramPaths::system());
        let spec = service.rsync_spec(
            OsString::from("/tmp/busy-nas-workspace/demo/"),
            OsString::from("nas.example:/srv/developer/demo/"),
            true,
            false,
        );
        assert_eq!(spec.program, PathBuf::from("rsync"));
        assert!(spec.args.contains(&OsString::from("--delete")));
        assert!(spec
            .args
            .windows(2)
            .any(|pair| pair == [OsString::from("--exclude"), OsString::from("node_modules/")]));
        assert_eq!(
            spec.args.last(),
            Some(&OsString::from("nas.example:/srv/developer/demo/"))
        );
    }
}
