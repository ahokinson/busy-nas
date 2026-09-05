use std::{
    ffi::OsString,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
};

use crate::{BusyNasError, Result};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandSpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub stdin: Option<Vec<u8>>,
}

impl CommandSpec {
    pub fn new(
        program: impl Into<PathBuf>,
        args: impl IntoIterator<Item = impl Into<OsString>>,
    ) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
            stdin: None,
        }
    }

    pub fn with_stdin(mut self, stdin: Vec<u8>) -> Self {
        self.stdin = Some(stdin);
        self
    }
}

#[derive(Clone, Debug)]
pub struct CommandOutput {
    pub success: bool,
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl CommandOutput {
    pub fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr).trim().to_owned()
    }

    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).to_string()
    }
}

pub trait CommandRunner {
    fn run(&mut self, spec: &CommandSpec) -> Result<CommandOutput>;
}

#[derive(Clone, Debug)]
pub struct ProcessRunner {
    pub ssh_program: PathBuf,
    pub rsync_program: PathBuf,
}

impl ProcessRunner {
    pub fn system() -> Self {
        Self::with_programs("ssh", "rsync")
    }

    pub fn with_programs(
        ssh_program: impl Into<PathBuf>,
        rsync_program: impl Into<PathBuf>,
    ) -> Self {
        Self {
            ssh_program: ssh_program.into(),
            rsync_program: rsync_program.into(),
        }
    }
}

impl CommandRunner for ProcessRunner {
    fn run(&mut self, spec: &CommandSpec) -> Result<CommandOutput> {
        let mut command = Command::new(&spec.program);
        command
            .args(&spec.args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if spec.stdin.is_some() {
            command.stdin(Stdio::piped());
        }
        let mut child = command
            .spawn()
            .map_err(|source| BusyNasError::ProcessStart {
                program: spec.program.display().to_string(),
                source,
            })?;
        if let Some(input) = &spec.stdin {
            child
                .stdin
                .as_mut()
                .expect("stdin is piped when input is supplied")
                .write_all(input)
                .map_err(BusyNasError::ConfigRead)?;
        }
        let output = child.wait_with_output().map_err(BusyNasError::ConfigRead)?;
        Ok(CommandOutput {
            success: output.status.success(),
            status: output.status.code().unwrap_or(-1),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}

pub fn require_success(spec: &CommandSpec, output: CommandOutput) -> Result<CommandOutput> {
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

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::CommandSpec;

    #[test]
    fn command_specs_keep_arguments_separate() {
        let spec = CommandSpec::new("ssh", ["--", "nas.example", "mkdir", "/srv/developer/a-b"]);
        assert_eq!(spec.program.to_string_lossy(), "ssh");
        assert_eq!(
            spec.args,
            vec![
                OsString::from("--"),
                OsString::from("nas.example"),
                OsString::from("mkdir"),
                OsString::from("/srv/developer/a-b")
            ]
        );
    }
}
