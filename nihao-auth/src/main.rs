use nihao_auth::{account_uid, authorize, AuthResponse, HELPER_TIMEOUT, MAX_RESPONSE_BYTES};
use nihao_core::config::{trusted_system_path, Config, TrustedPathKind};
use nihao_core::{password::PasswordStore, FaceRecognizer};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

fn prepare_process() -> Result<(), String> {
    // Scrub here as well as in PAM: any local process can invoke this executable.
    for (name, _) in std::env::vars_os() {
        std::env::remove_var(name);
    }
    std::env::set_current_dir("/").map_err(|e| e.to_string())?;
    unsafe {
        libc::umask(0o077);
        let limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        if libc::setrlimit(libc::RLIMIT_CORE, &limit) != 0 {
            return Err("Cannot disable authentication core dumps".into());
        }
        // Do not allow a caller to attach after the helper gains privilege.
        if libc::prctl(libc::PR_SET_DUMPABLE, 0) != 0 {
            return Err("Cannot protect authentication process".into());
        }
        if libc::geteuid() != 0 {
            return Err("Authentication helper requires root privilege".into());
        }
    }
    configure_lifecycle(HELPER_TIMEOUT.as_secs() as u32)
}

fn configure_lifecycle(timeout_secs: u32) -> Result<(), String> {
    unsafe {
        // Bound direct invocations too, and stop immediately if the locker is
        // cancelled or exits while authentication is in progress.
        let parent = libc::getppid();
        if parent == 1
            || libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0
            || libc::getppid() != parent
        {
            return Err("Authentication caller exited".into());
        }
        libc::signal(libc::SIGALRM, libc::SIG_DFL);
        let mut signals = std::mem::zeroed();
        libc::sigemptyset(&mut signals);
        libc::sigaddset(&mut signals, libc::SIGALRM);
        if libc::sigprocmask(libc::SIG_UNBLOCK, &signals, std::ptr::null_mut()) != 0 {
            return Err("Cannot enable authentication watchdog".into());
        }
        libc::alarm(timeout_secs);
    }
    Ok(())
}

fn authenticate() -> Result<AuthResponse, String> {
    let mut args = std::env::args_os().skip(1);
    let username = args
        .next()
        .ok_or("An account name is required")?
        .into_string()
        .map_err(|_| "Invalid account name")?;
    if args.next().is_some() {
        return Err("Unexpected authentication helper arguments".into());
    }
    let caller_uid = unsafe { libc::getuid() };
    authorize(caller_uid, account_uid(&username)?)?;
    // Serialize requests before allocating models or opening the camera.
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open("/run/nihao-auth.lock")
        .map_err(|e| e.to_string())?;
    let meta = lock.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.uid() != 0 || meta.mode() & 0o077 != 0 {
        return Err("Untrusted authentication lock".into());
    }
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err("Another face authentication is in progress".into());
    }
    let config = Config::load_system().map_err(|e| e.to_string())?;
    validate_enrollment(&config, &username)?;
    let mut recognizer = FaceRecognizer::new(config).map_err(|e| e.to_string())?;
    if !recognizer
        .authenticate(&username)
        .map_err(|e| e.to_string())?
    {
        return Err("Face not recognized".into());
    }
    // An unprivileged application must never be able to retrieve the account's
    // stored password by invoking this helper while the user faces the camera.
    let token = if caller_uid == 0 {
        let store = PasswordStore::new("/etc/nihao");
        if store.has_password(&username).map_err(|e| e.to_string())? {
            trusted_system_path(
                &Path::new("/etc/nihao").join(format!("{username}.key")),
                TrustedPathKind::File,
            )
            .map_err(|e| e.to_string())?;
            match store.load_password(&username) {
                Ok(password) => Some(password),
                Err(e) => {
                    log::warn!("Cannot load service unlock password: {e}");
                    None
                }
            }
        } else {
            None
        }
    } else {
        None
    };
    Ok(AuthResponse {
        version: 1,
        username,
        authenticated: true,
        token,
    })
}

fn validate_enrollment(config: &Config, username: &str) -> Result<(), String> {
    let dir = config.storage.database_path.join(username);
    trusted_system_path(&dir, TrustedPathKind::Directory).map_err(|e| e.to_string())?;
    trusted_system_path(&dir.join("metadata.toml"), TrustedPathKind::File)
        .map_err(|e| e.to_string())?;
    let store = nihao_core::store::FaceStore::new(&config.storage.database_path);
    for face in store.list_faces(username).map_err(|e| e.to_string())? {
        // A metadata entry must be a filename, never a path to another account.
        nihao_auth::validate_username(&face.id)?;
        trusted_system_path(&dir.join(format!("{}.bin", face.id)), TrustedPathKind::File)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn main() {
    let result = std::panic::catch_unwind(|| {
        prepare_process()?;
        let _ = syslog::init_unix(syslog::Facility::LOG_AUTH, log::LevelFilter::Warn);
        let response = authenticate()?;
        let bytes = serde_json::to_vec(&response).map_err(|e| e.to_string())?;
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err("Authentication response too large".into());
        }
        std::io::stdout()
            .write_all(&bytes)
            .map_err(|e| e.to_string())
    });
    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            log::warn!("NiHao helper: {error}");
            std::process::exit(1);
        }
        Err(_) => {
            log::error!("NiHao helper panicked");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    #[test]
    fn watchdog_cannot_be_disabled_by_inherited_signal_state() {
        if std::env::var_os("NIHAO_WATCHDOG_TEST_CHILD").is_some() {
            unsafe {
                libc::signal(libc::SIGALRM, libc::SIG_IGN);
                let mut blocked = std::mem::zeroed();
                libc::sigemptyset(&mut blocked);
                libc::sigaddset(&mut blocked, libc::SIGALRM);
                libc::sigprocmask(libc::SIG_BLOCK, &blocked, std::ptr::null_mut());
            }
            configure_lifecycle(1).unwrap();
            std::thread::sleep(std::time::Duration::from_secs(3));
            panic!("watchdog did not terminate the authentication process");
        }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tests::watchdog_cannot_be_disabled_by_inherited_signal_state",
            ])
            .env("NIHAO_WATCHDOG_TEST_CHILD", "1")
            .output()
            .unwrap();
        assert_eq!(output.status.signal(), Some(libc::SIGALRM));
    }
}
