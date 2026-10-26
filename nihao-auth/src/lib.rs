//! The bounded protocol between PAM and the privileged face verifier.
use serde::{Deserialize, Serialize};
use std::ffi::CString;
use std::fs;
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const HELPER_PATH: &str = "/usr/lib/nihao/nihao-auth";
pub const MAX_RESPONSE_BYTES: usize = 16 * 1024;
pub const HELPER_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthResponse {
    pub version: u32,
    pub username: String,
    pub authenticated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

pub fn validate_username(username: &str) -> Result<(), String> {
    if username.is_empty()
        || username.len() > 256
        || username == "."
        || username == ".."
        || username.chars().any(|c| c == '/' || c.is_control())
    {
        return Err("Invalid account name".into());
    }
    Ok(())
}

/// Resolve identity through NSS, never through USER or SUDO_USER.
pub fn account_uid(username: &str) -> Result<u32, String> {
    validate_username(username)?;
    let name = CString::new(username).map_err(|_| "Invalid account name")?;
    let mut buffer = vec![0u8; 16 * 1024];
    loop {
        let mut account = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut found = std::ptr::null_mut();
        // The scratch buffer and result remain alive until the UID has been copied.
        let status = unsafe {
            libc::getpwnam_r(
                name.as_ptr(),
                account.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut found,
            )
        };
        if status == libc::ERANGE && buffer.len() < 1024 * 1024 {
            buffer.resize(buffer.len() * 2, 0);
            continue;
        }
        if status != 0 || found.is_null() {
            return Err("Unknown or unavailable account".into());
        }
        return Ok(unsafe { account.assume_init() }.pw_uid);
    }
}

pub fn authorize(caller_uid: u32, target_uid: u32) -> Result<(), String> {
    if caller_uid == 0 || caller_uid == target_uid {
        Ok(())
    } else {
        Err("Face authentication is restricted to the calling account".into())
    }
}

fn validate_helper(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("Authentication helper path must be absolute".into());
    }
    if !fs::symlink_metadata(path)
        .map_err(|e| e.to_string())?
        .is_file()
    {
        return Err("Authentication helper must be a regular file".into());
    }
    // Check both the spelled path and resolved path so an untrusted parent cannot
    // replace a symlink after validation.
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("Helper unavailable: {e}"))?;
    for candidate in [path, canonical.as_path()] {
        for ancestor in candidate.ancestors() {
            let meta = fs::symlink_metadata(ancestor).map_err(|e| e.to_string())?;
            if meta.uid() != 0 || (!meta.file_type().is_symlink() && meta.mode() & 0o022 != 0) {
                return Err(
                    "Authentication helper has an untrusted owner or writable parent".into(),
                );
            }
        }
    }
    let meta = fs::metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.mode() & 0o4000 == 0 || meta.mode() & 0o111 == 0 {
        return Err("Authentication helper must be a root-owned setuid executable".into());
    }
    Ok(canonical)
}

pub fn authenticate(username: &str) -> Result<AuthResponse, String> {
    validate_username(username)?;
    let path = validate_helper(Path::new(HELPER_PATH))?;
    let mut command = Command::new(path);
    command.arg(username).env_clear().current_dir("/");
    let allow_token = unsafe { libc::geteuid() } == 0;
    // A privileged PAM host (e.g. sudo) may still have the invoking user's real
    // UID. Normalize it only with existing root privilege, so the helper can
    // distinguish that host from an ordinary desktop application.
    if allow_token {
        command.uid(0);
    }
    let bytes = collect_response(&mut command, HELPER_TIMEOUT)?;
    decode_response(&bytes, username, allow_token)
}

fn decode_response(
    bytes: &[u8],
    username: &str,
    allow_token: bool,
) -> Result<AuthResponse, String> {
    let response: AuthResponse =
        serde_json::from_slice(bytes).map_err(|_| "Invalid authentication helper response")?;
    if response.version != 1 || response.username != username || !response.authenticated {
        return Err("Face authentication did not succeed for the requested account".into());
    }
    if response.token.is_some() && !allow_token {
        return Err("Authentication helper returned a token to an unprivileged caller".into());
    }
    Ok(response)
}

fn collect_response(command: &mut Command, timeout: Duration) -> Result<Vec<u8>, String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Cannot start authentication helper: {e}"))?;
    let mut stdout = child.stdout.take().expect("piped stdout");
    let result = (|| {
        let fd = stdout.as_raw_fd();
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err("Cannot configure authentication helper pipe".into());
        }
        let start = Instant::now();
        let mut output = Vec::new();
        let mut status = None;
        loop {
            let mut chunk = [0u8; 4096];
            match stdout.read(&mut chunk) {
                Ok(0) if status.is_some() => break,
                Ok(0) => {}
                Ok(count) => {
                    if output.len() + count > MAX_RESPONSE_BYTES {
                        return Err("Authentication helper response exceeded its size limit".into());
                    }
                    output.extend_from_slice(&chunk[..count]);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    if status.is_some() {
                        break;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(format!("Cannot read authentication helper response: {e}")),
            }
            if status.is_none() {
                status = child.try_wait().map_err(|e| e.to_string())?;
            }
            if start.elapsed() >= timeout {
                return Err("Face authentication helper timed out".into());
            }
            if status.is_none() {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        if !status.expect("child exited").success() {
            return Err("Face authentication helper failed; use password authentication".into());
        }
        Ok(output)
    })();
    if result.is_err() {
        let _ = child.kill();
    }
    // The verifier normalizes its UIDs after authorization, so an unprivileged
    // PAM host may not be allowed to signal it. Its independent alarm bounds
    // this reap, and parent-death SIGKILL covers cancellation of the PAM host.
    let _ = child.wait();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caller_cannot_authenticate_another_account() {
        assert!(authorize(1000, 1000).is_ok());
        assert!(authorize(0, 1000).is_ok());
        assert!(authorize(1000, 0).is_err());
        assert!(authorize(1000, 1001).is_err());
    }

    #[test]
    fn rejects_path_traversal_and_control_characters() {
        for name in ["", ".", "..", "/root", "../root", "user\n", "user\0"] {
            assert!(validate_username(name).is_err(), "{name:?}");
        }
        assert!(validate_username("johnny").is_ok());
    }

    #[test]
    fn validates_response_identity_and_privilege() {
        let response = br#"{"version":1,"username":"johnny","authenticated":true}"#;
        assert!(decode_response(response, "johnny", false).is_ok());
        assert!(decode_response(response, "root", true).is_err());
        assert!(decode_response(b"{}", "johnny", true).is_err());
        assert!(decode_response(b"not json", "johnny", true).is_err());
        let token = br#"{"version":1,"username":"johnny","authenticated":true,"token":"fake"}"#;
        assert!(decode_response(token, "johnny", false).is_err());
        assert!(decode_response(token, "johnny", true).is_ok());
    }

    #[test]
    fn handles_crash_oversized_output_and_timeout() {
        let mut failure = Command::new("/bin/sh");
        failure.args(["-c", "printf '{}'; exit 1"]);
        assert!(collect_response(&mut failure, Duration::from_secs(1)).is_err());
        let mut flood = Command::new("/usr/bin/head");
        flood.args(["-c", "20000", "/dev/zero"]);
        assert!(collect_response(&mut flood, Duration::from_secs(1))
            .unwrap_err()
            .contains("size limit"));
        let mut stalled = Command::new("/bin/sleep");
        stalled.arg("5");
        let start = Instant::now();
        assert!(collect_response(&mut stalled, Duration::from_millis(40))
            .unwrap_err()
            .contains("timed out"));
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn collects_successful_output() {
        let mut command = Command::new("/usr/bin/printf");
        command.arg("response");
        assert_eq!(
            collect_response(&mut command, Duration::from_secs(1)).unwrap(),
            b"response"
        );
    }
}
