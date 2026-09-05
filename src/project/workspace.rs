use std::{fs, path::Path};

use crate::{BusyNasError, Result};

/// The local worktree must remain local.
pub fn validate_local_workspace(root: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(root)
        .map_err(|error| BusyNasError::UnsafeWorkspace(format!("{} ({error})", root.display())))?;
    if metadata.file_type().is_symlink() {
        return Err(BusyNasError::UnsafeWorkspace(format!(
            "{} is a symlink",
            root.display()
        )));
    }
    if !metadata.is_dir() {
        return Err(BusyNasError::UnsafeWorkspace(format!(
            "{} is not a directory",
            root.display()
        )));
    }

    let canonical = root
        .canonicalize()
        .map_err(|error| BusyNasError::UnsafeWorkspace(format!("{} ({error})", root.display())))?;
    #[cfg(target_os = "linux")]
    reject_linux_network_mount(&canonical)?;
    #[cfg(target_os = "macos")]
    reject_macos_network_mount(&canonical)?;

    Ok(())
}

#[cfg(target_os = "linux")]
fn reject_linux_network_mount(path: &Path) -> Result<()> {
    let contents = fs::read_to_string("/proc/self/mountinfo").map_err(|error| {
        BusyNasError::UnsafeWorkspace(format!("cannot inspect mount information ({error})"))
    })?;
    let mut matched: Option<(usize, String)> = None;
    for line in contents.lines() {
        let Some((before_separator, after_separator)) = line.split_once(" - ") else {
            continue;
        };
        let mut fields = before_separator.split_whitespace();
        let _mount_id = fields.next();
        let _parent_id = fields.next();
        let _major_minor = fields.next();
        let _root = fields.next();
        let Some(mount_point) = fields.next() else {
            continue;
        };
        let mount_point = unescape_mount_path(mount_point);
        let file_system = after_separator
            .split_whitespace()
            .next()
            .unwrap_or_default();
        if path.starts_with(&mount_point) {
            let length = mount_point.as_os_str().len();
            if matched.as_ref().map_or(true, |(best, _)| length > *best) {
                matched = Some((length, file_system.to_owned()));
            }
        }
    }

    if let Some((_, file_system)) = matched {
        if matches!(
            file_system.as_str(),
            "nfs" | "nfs4" | "cifs" | "smb3" | "fuse.sshfs"
        ) {
            return Err(BusyNasError::UnsafeWorkspace(format!(
                "{} is on a {file_system} mount",
                path.display()
            )));
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn unescape_mount_path(value: &str) -> std::path::PathBuf {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\'
            && index + 3 < bytes.len()
            && bytes[index + 1..index + 4].iter().all(u8::is_ascii_digit)
        {
            let octal =
                std::str::from_utf8(&bytes[index + 1..index + 4]).expect("mountinfo is UTF-8");
            if let Ok(character) = u8::from_str_radix(octal, 8) {
                output.push(character);
                index += 4;
                continue;
            }
        }
        output.push(bytes[index]);
        index += 1;
    }
    std::path::PathBuf::from(String::from_utf8_lossy(&output).into_owned())
}

#[cfg(target_os = "macos")]
fn reject_macos_network_mount(path: &Path) -> Result<()> {
    use std::{
        ffi::{CStr, CString},
        mem::MaybeUninit,
    };

    let path = CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|_| BusyNasError::UnsafeWorkspace("workspace path contains a NUL byte".into()))?;
    let mut stats = MaybeUninit::<libc::statfs>::zeroed();
    // SAFETY: `path` is NUL terminated and `stats` points to valid writable memory.
    let result = unsafe { libc::statfs(path.as_ptr(), stats.as_mut_ptr()) };
    if result != 0 {
        let error = std::io::Error::last_os_error();
        return Err(BusyNasError::UnsafeWorkspace(format!(
            "cannot inspect workspace filesystem ({error})"
        )));
    }
    // SAFETY: a successful statfs call initializes the entire struct.
    let stats = unsafe { stats.assume_init() };
    // SAFETY: f_fstypename is a NUL-terminated field supplied by statfs.
    let filesystem = unsafe { CStr::from_ptr(stats.f_fstypename.as_ptr()) }
        .to_string_lossy()
        .to_ascii_lowercase();
    if matches!(filesystem.as_str(), "nfs" | "smbfs" | "webdav") {
        return Err(BusyNasError::UnsafeWorkspace(format!(
            "workspace is on a {filesystem} mount"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    #[test]
    fn mount_path_unescaping_handles_spaces() {
        use super::unescape_mount_path;
        assert_eq!(
            unescape_mount_path("/tmp/a\\040b"),
            std::path::PathBuf::from("/tmp/a b")
        );
    }

    #[cfg(unix)]
    #[test]
    fn plain_local_temp_directory_is_a_safe_workspace() {
        use super::validate_local_workspace;
        let temporary = tempfile::tempdir().unwrap();
        validate_local_workspace(temporary.path()).unwrap();
    }
}
