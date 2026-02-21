use lazy_static::lazy_static;
use nihao_core::{config::Config, password::PasswordStore, FaceRecognizer};
use pamsm::{Pam, PamError, PamFlag, PamLibExt, PamServiceModule};
use std::ffi::CString;
use std::panic;
use std::sync::Mutex;
use std::time::Duration;

lazy_static! {
    /// Global recognizer instance with lazy initialization
    /// This allows model loading to happen once and be reused across authentication attempts
    static ref RECOGNIZER: Mutex<Option<FaceRecognizer>> = Mutex::new(None);
}

struct PamNihao;

impl PamServiceModule for PamNihao {
    fn authenticate(pamh: Pam, _flags: PamFlag, _args: Vec<String>) -> PamError {
        // Initialize syslog (ignore errors)
        // Use Warn level in release builds to reduce log noise
        #[cfg(debug_assertions)]
        let log_level = log::LevelFilter::Info;
        #[cfg(not(debug_assertions))]
        let log_level = log::LevelFilter::Warn;

        let _ = syslog::init_unix(syslog::Facility::LOG_AUTH, log_level);

        // CRITICAL: Wrap everything in catch_unwind to prevent panics from crossing FFI boundary
        let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
            authenticate_impl(&pamh)
        }));

        match result {
            Ok(Ok(())) => {
                log::info!("NiHao: Authentication succeeded");
                PamError::SUCCESS
            }
            Ok(Err(e)) => {
                log::warn!("NiHao: Authentication failed: {}", e);
                PamError::AUTH_ERR
            }
            Err(_) => {
                log::error!("NiHao: Panic caught during authentication! Falling through to password");
                PamError::SERVICE_ERR
            }
        }
    }

    fn setcred(_pamh: Pam, _flags: PamFlag, _args: Vec<String>) -> PamError {
        PamError::SUCCESS
    }

    fn acct_mgmt(_pamh: Pam, _flags: PamFlag, _args: Vec<String>) -> PamError {
        PamError::SUCCESS
    }
}

/// Internal authentication implementation
/// This is separate to allow catch_unwind to work properly
fn authenticate_impl(pamh: &Pam) -> Result<(), String> {
    // NOTE: We don't redirect stdout/stderr because it affects the calling process
    // Instead, we ensure zero prints in our code (verified by audit) and use syslog only

    // Get the actual invoking user, not the target user
    // For sudo: SUDO_USER contains the real user, PAM_USER contains "root"
    // For lock screen, login, etc.: PAM_USER (via get_user) is the correct user
    let user = std::env::var("SUDO_USER")
        .ok()
        .or_else(|| {
            pamh.get_user(None)
                .ok()
                .flatten()
                .map(|cstr| cstr.to_string_lossy().into_owned())
        })
        .ok_or_else(|| "Failed to determine username".to_string())?;

    log::info!("NiHao: Attempting facial authentication for user: {}", user);

    // Load configuration
    let config = Config::load().map_err(|e| format!("Failed to load config: {}", e))?;

    // Get or initialize recognizer
    let mut recognizer_lock = RECOGNIZER
        .lock()
        .map_err(|e| format!("Failed to lock recognizer: {}", e))?;

    if recognizer_lock.is_none() {
        log::debug!("NiHao: Initializing face recognizer (first use)");
        let recognizer = FaceRecognizer::new(config.clone())
            .map_err(|e| format!("Failed to create recognizer: {}", e))?;
        *recognizer_lock = Some(recognizer);
    }

    let recognizer = recognizer_lock
        .as_mut()
        .ok_or_else(|| "Recognizer not initialized".to_string())?;

    // Check if user has enrolled faces
    if !recognizer.store().has_faces(&user) {
        log::info!("NiHao: No enrolled faces for user {}, falling through", user);
        return Err("No enrolled faces".to_string());
    }

    // Set timeout for authentication
    let start = std::time::Instant::now();
    let timeout = Duration::from_secs(config.matching.timeout_secs);

    // Authenticate with timeout check
    let auth_result = recognizer.authenticate(&user);

    if start.elapsed() > timeout {
        log::warn!("NiHao: Authentication timeout");
        return Err("Timeout".to_string());
    }

    match auth_result {
        Ok(true) => {
            log::info!("NiHao: Face recognized for user: {}", user);

            // Try to set PAM_AUTHTOK for automatic service unlock (KWallet, GNOME Keyring, etc.)
            let password_store = PasswordStore::new("/etc/nihao");
            if password_store.has_password(&user) {
                match password_store.load_password(&user) {
                    Ok(password) => {
                        // Convert password to CString for PAM
                        match CString::new(password) {
                            Ok(c_password) => {
                                // Set PAM_AUTHTOK
                                match pamh.set_authtok(&c_password) {
                                    Ok(_) => {
                                        log::info!("NiHao: PAM_AUTHTOK set successfully for service unlock");
                                    }
                                    Err(e) => {
                                        log::warn!("NiHao: Failed to set PAM_AUTHTOK: {:?}", e);
                                        // Don't fail auth if we can't set the token
                                    }
                                }
                            }
                            Err(e) => {
                                log::warn!("NiHao: Failed to convert password to CString: {}", e);
                            }
                        }
                    }
                    Err(e) => {
                        log::warn!("NiHao: Failed to load stored password: {}", e);
                        // Don't fail auth if we can't load the password
                    }
                }
            } else {
                log::debug!("NiHao: No stored password for user {}, services won't auto-unlock", user);
            }

            Ok(())
        }
        Ok(false) => {
            log::info!("NiHao: Face not recognized for user: {}", user);
            Err("Face not recognized".to_string())
        }
        Err(e) => {
            log::warn!("NiHao: Authentication error: {}", e);
            Err(format!("Authentication error: {}", e))
        }
    }
}

pamsm::pam_module!(PamNihao);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_module_compiles() {
        // Just verify the module compiles
        // Actual PAM testing requires pamtester
    }
}
