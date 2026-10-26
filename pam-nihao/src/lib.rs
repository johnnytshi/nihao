//! PAM stays in the caller's security context; privileged work is delegated.
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};

// Linux-PAM constants from security/_pam_types.h. Use C integers at the ABI
// boundary: Rust enums cannot represent arbitrary PAM flag combinations.
const PAM_SUCCESS: c_int = 0;
const PAM_SERVICE_ERR: c_int = 3;
const PAM_AUTH_ERR: c_int = 7;
const PAM_AUTHTOK: c_int = 6;

#[link(name = "pam")]
extern "C" {
    fn pam_get_user(pamh: *mut c_void, user: *mut *const c_char, prompt: *const c_char) -> c_int;
    fn pam_set_item(pamh: *mut c_void, item: c_int, value: *const c_void) -> c_int;
}

unsafe fn authenticate_impl(pamh: *mut c_void) -> Result<(), String> {
    if pamh.is_null() {
        return Err("Missing PAM handle".into());
    }
    let mut user = std::ptr::null();
    if pam_get_user(pamh, &mut user, std::ptr::null()) != PAM_SUCCESS || user.is_null() {
        return Err("Cannot determine PAM account".into());
    }
    let username = CStr::from_ptr(user)
        .to_str()
        .map_err(|_| "Invalid PAM account name")?;
    let result = nihao_auth::authenticate(username)?;
    if let Some(token) = result.token {
        let token = CString::new(token).map_err(|_| "Invalid stored service password")?;
        if pam_set_item(pamh, PAM_AUTHTOK, token.as_ptr().cast()) != PAM_SUCCESS {
            log::warn!("NiHao: cannot set service unlock password");
        }
    }
    Ok(())
}

/// # Safety
/// `pamh` must be a live Linux-PAM handle, or null for a rejected request.
#[no_mangle]
pub unsafe extern "C" fn pam_sm_authenticate(
    pamh: *mut c_void,
    _flags: c_int,
    _argc: c_int,
    _argv: *const *const c_char,
) -> c_int {
    let _ = syslog::init_unix(syslog::Facility::LOG_AUTH, log::LevelFilter::Warn);
    match std::panic::catch_unwind(|| authenticate_impl(pamh)) {
        Ok(Ok(())) => PAM_SUCCESS,
        Ok(Err(error)) => {
            log::warn!("NiHao: Authentication failed: {error}");
            PAM_AUTH_ERR
        }
        Err(_) => {
            log::error!("NiHao: authentication panicked");
            PAM_SERVICE_ERR
        }
    }
}

macro_rules! callback {
    ($name:ident, $result:expr) => {
        #[no_mangle]
        pub extern "C" fn $name(
            _pamh: *mut c_void,
            _flags: c_int,
            _argc: c_int,
            _argv: *const *const c_char,
        ) -> c_int {
            $result
        }
    };
}

callback!(pam_sm_setcred, PAM_SUCCESS);
callback!(pam_sm_acct_mgmt, PAM_SUCCESS);
callback!(pam_sm_open_session, PAM_SERVICE_ERR);
callback!(pam_sm_close_session, PAM_SERVICE_ERR);
callback!(pam_sm_chauthtok, PAM_SERVICE_ERR);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c_callbacks_return_full_integer_statuses() {
        let authenticate: unsafe extern "C" fn(
            *mut c_void,
            c_int,
            c_int,
            *const *const c_char,
        ) -> c_int = pam_sm_authenticate;
        let status = unsafe { authenticate(std::ptr::null_mut(), 0, 0, std::ptr::null()) };
        assert_eq!(status, PAM_AUTH_ERR);
        assert_eq!(
            pam_sm_setcred(std::ptr::null_mut(), 0, 0, std::ptr::null()),
            PAM_SUCCESS
        );
    }
}
