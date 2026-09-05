#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use busy_nas::{
    config::Config,
    paths::AppPaths,
    process::ProcessRunner,
    project::ProjectName,
    service::{ProgramPaths, Service},
    state, BusyNasError,
};
use tempfile::TempDir;

struct Harness {
    _temporary: TempDir,
    remote_root: PathBuf,
    workspace: PathBuf,
    paths: AppPaths,
    config: Config,
    ssh: PathBuf,
    rsync: PathBuf,
}

impl Harness {
    fn new(_name: &str, remote_root: Option<PathBuf>, tools: Option<(PathBuf, PathBuf)>) -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let remote_root = remote_root.unwrap_or_else(|| temporary.path().join("remote"));
        let workspace = temporary.path().join("workspace");
        let state_dir = temporary.path().join("state");
        fs::create_dir_all(&remote_root).unwrap();
        fs::create_dir_all(&workspace).unwrap();

        let (ssh, rsync) = tools.unwrap_or_else(|| install_fake_tools(temporary.path()));
        let config = Config::parse(&format!(
            r#"
                workspace_root = "{}"
                snapshot_retention = 20
                [nas]
                host = "fake-nas"
                user = "developer"
                root = "{}"
            "#,
            workspace.display(),
            remote_root.display(),
        ))
        .unwrap();
        Self {
            _temporary: temporary,
            remote_root,
            workspace,
            paths: AppPaths {
                config_file: PathBuf::from("unused.toml"),
                state_dir,
            },
            config,
            ssh,
            rsync,
        }
    }

    fn service<'a>(&'a self, runner: &'a mut ProcessRunner) -> Service<'a, ProcessRunner> {
        Service::new(
            &self.config,
            &self.paths,
            runner,
            ProgramPaths {
                ssh: self.ssh.clone(),
                rsync: self.rsync.clone(),
            },
        )
    }

    fn runner(&self) -> ProcessRunner {
        ProcessRunner::with_programs(&self.ssh, &self.rsync)
    }

    fn remote_project(&self, project: &str) -> PathBuf {
        self.remote_root.join(project)
    }

    fn local_project(&self, project: &str) -> PathBuf {
        self.workspace.join(project)
    }
}

fn project() -> ProjectName {
    ProjectName::parse("demo").unwrap()
}

fn install_fake_tools(root: &Path) -> (PathBuf, PathBuf) {
    let tools = root.join("fake-tools");
    fs::create_dir_all(&tools).unwrap();
    let ssh = tools.join("ssh");
    let rsync = tools.join("rsync");
    write_executable(
        &ssh,
        r#"#!/bin/sh
set -eu
if [ "$1" = "--" ]; then shift; fi
shift
tool_dir=$(dirname "$0")
if [ "$1" = "rsync" ]; then
  shift
  exec "$tool_dir/rsync" "$@"
fi
exec "$@"
"#,
    );
    write_executable(
        &rsync,
        r#"#!/bin/sh
set -eu
if [ -e "$0.fail" ]; then
  echo "intentional fake-rsync failure" >&2
  exit 23
fi

delete=0
dry_run=0
filtered=0
while [ "$#" -gt 2 ]; do
  case "$1" in
    -a|--protect-args|--itemize-changes) shift ;;
    --delete) delete=1; shift ;;
    --dry-run) dry_run=1; shift ;;
    --exclude) filtered=1; shift 2 ;;
    -e) shift 2 ;;
    *) break ;;
  esac
done
source_path=$1
destination_path=$2
case "$source_path" in *:*) source_path=${source_path#*:} ;; esac
case "$destination_path" in *:*) destination_path=${destination_path#*:} ;; esac
source_path=${source_path%/}
destination_path=${destination_path%/}

if [ "$dry_run" -eq 1 ]; then
  exit 0
fi

excluded() {
  case "$1" in
    node_modules|node_modules/*|target|target/*|.direnv|.direnv/*|.devenv|.devenv/*|result|result-*|result-*/*) return 0 ;;
    *) return 1 ;;
  esac
}

mkdir -p "$destination_path"
find "$source_path" -mindepth 1 -print | while IFS= read -r entry; do
  relative=${entry#"$source_path"/}
  if [ "$filtered" -eq 1 ] && excluded "$relative"; then continue; fi
  if [ -d "$entry" ]; then
    mkdir -p "$destination_path/$relative"
  else
    mkdir -p "$(dirname "$destination_path/$relative")"
    cp -a "$entry" "$destination_path/$relative"
  fi
done

if [ "$delete" -eq 1 ]; then
  find "$destination_path" -mindepth 1 -depth -print | while IFS= read -r entry; do
    relative=${entry#"$destination_path"/}
    if [ "$filtered" -eq 1 ] && excluded "$relative"; then continue; fi
    if [ ! -e "$source_path/$relative" ] && [ ! -L "$source_path/$relative" ]; then
      rm -rf "$entry"
    fi
  done
fi
"#,
    );
    (ssh, rsync)
}

fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

#[test]
fn get_then_put_transfers_source_and_releases_lease() {
    let harness = Harness::new("one", None, None);
    let project = project();
    fs::create_dir_all(harness.remote_project("demo").join(".git")).unwrap();
    fs::write(harness.remote_project("demo").join("hello.txt"), "from-nas").unwrap();
    fs::write(
        harness.remote_project("demo").join(".git/index"),
        "git-data",
    )
    .unwrap();

    let mut runner = harness.runner();
    harness.service(&mut runner).get(&project).unwrap();
    assert_eq!(
        fs::read_to_string(harness.local_project("demo").join("hello.txt")).unwrap(),
        "from-nas"
    );
    assert!(harness
        .remote_root
        .join(".busy-nas/leases/demo/token")
        .is_file());

    fs::write(
        harness.local_project("demo").join("hello.txt"),
        "from-local",
    )
    .unwrap();
    harness.service(&mut runner).put(&project).unwrap();
    assert_eq!(
        fs::read_to_string(harness.remote_project("demo").join("hello.txt")).unwrap(),
        "from-local"
    );
    assert!(!harness.local_project("demo").exists());
    assert!(!harness.remote_root.join(".busy-nas/leases/demo").exists());
    assert!(state::read(&harness.paths, &project).is_err());
}

#[test]
fn failed_get_removes_partial_checkout_and_new_lease() {
    let harness = Harness::new("partial", None, None);
    let project = project();
    fs::create_dir_all(harness.remote_project("demo")).unwrap();
    fs::write(harness.remote_project("demo").join("hello.txt"), "from-nas").unwrap();
    fs::write(harness.rsync.with_file_name("rsync.fail"), "fail").unwrap();

    let mut runner = harness.runner();
    assert!(harness.service(&mut runner).get(&project).is_err());
    assert!(!harness.local_project("demo").exists());
    assert!(!harness.remote_root.join(".busy-nas/leases/demo").exists());
    assert!(state::read(&harness.paths, &project).is_err());
}

#[test]
fn a_second_machine_cannot_acquire_an_active_lease() {
    let first = Harness::new("first", None, None);
    let project = project();
    fs::create_dir_all(first.remote_project("demo")).unwrap();
    fs::write(first.remote_project("demo").join("hello.txt"), "source").unwrap();
    let second = Harness::new(
        "second",
        Some(first.remote_root.clone()),
        Some((first.ssh.clone(), first.rsync.clone())),
    );

    let mut first_runner = first.runner();
    first.service(&mut first_runner).get(&project).unwrap();
    let mut second_runner = second.runner();
    let error = second
        .service(&mut second_runner)
        .get(&project)
        .unwrap_err();
    assert!(matches!(error, BusyNasError::LeaseHeld(name) if name == "demo"));
}

#[test]
fn put_snapshots_before_sync_propagates_deletions_and_preserves_exclusions() {
    let harness = Harness::new("snapshot", None, None);
    let project = project();
    fs::create_dir_all(harness.remote_project("demo").join("target")).unwrap();
    fs::write(harness.remote_project("demo").join("hello.txt"), "before").unwrap();
    fs::write(
        harness.remote_project("demo").join("remove-me.txt"),
        "remove me",
    )
    .unwrap();
    fs::write(
        harness.remote_project("demo").join("target/cache"),
        "machine-build",
    )
    .unwrap();

    let mut runner = harness.runner();
    harness.service(&mut runner).get(&project).unwrap();
    let local = harness.local_project("demo");
    assert!(!local.join("target").exists());
    fs::write(local.join("hello.txt"), "after").unwrap();
    fs::remove_file(local.join("remove-me.txt")).unwrap();
    harness.service(&mut runner).put(&project).unwrap();

    assert_eq!(
        fs::read_to_string(harness.remote_project("demo").join("hello.txt")).unwrap(),
        "after"
    );
    assert!(!harness
        .remote_project("demo")
        .join("remove-me.txt")
        .exists());
    assert_eq!(
        fs::read_to_string(harness.remote_project("demo").join("target/cache")).unwrap(),
        "machine-build"
    );

    let snapshots = harness.remote_root.join(".busy-nas/snapshots/demo");
    let snapshot = fs::read_dir(snapshots)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(
        fs::read_to_string(snapshot.join("hello.txt")).unwrap(),
        "before"
    );
    assert!(snapshot.join("remove-me.txt").exists());
}

#[test]
fn forced_reclaim_replaces_a_stale_lease_and_get_resumes_it() {
    let harness = Harness::new("reclaim", None, None);
    let project = project();
    fs::create_dir_all(harness.remote_project("demo")).unwrap();
    fs::write(harness.remote_project("demo").join("hello.txt"), "source").unwrap();
    let stale = harness.remote_root.join(".busy-nas/leases/demo");
    fs::create_dir_all(&stale).unwrap();
    fs::write(stale.join("token"), "lost-machine-token\n").unwrap();

    let mut runner = harness.runner();
    harness
        .service(&mut runner)
        .reclaim(&project, true)
        .unwrap();
    let lease = state::read(&harness.paths, &project).unwrap();
    assert_ne!(lease.token, "lost-machine-token");
    assert_eq!(
        fs::read_to_string(stale.join("token")).unwrap().trim(),
        lease.token
    );

    harness.service(&mut runner).get(&project).unwrap();
    assert!(harness.local_project("demo").is_dir());
}
