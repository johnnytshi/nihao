//! Temporary illumination for the verified ASUS USB IR camera.
//!
//! Microsoft-XU selector 6 has one entry for streaming interface 3: D0 disables
//! face authentication, while D1 alternates illuminated and ambient frames.
//! https://learn.microsoft.com/en-us/windows-hardware/drivers/stream/uvc-extensions-1-5

use std::fs::{self, File};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::Path;

const UNIT: u8 = 14;
const SELECTOR: u8 = 6;
const STREAM_INTERFACE: u8 = 3;
const PAYLOAD_LEN: usize = 9;
const SET_CUR: u8 = 0x01;
const GET_CUR: u8 = 0x81;
const GET_MAX: u8 = 0x83;
const GET_LEN: u8 = 0x85;
const GET_INFO: u8 = 0x86;
const DISABLED: u8 = 1;
const ALTERNATING: u8 = 2;
const MS_CAMERA_CONTROL_GUID: [u8; 16] = [
    0xdc, 0x95, 0x3f, 0x0f, 0x32, 0x26, 0x4e, 0x4c, 0x92, 0xc9, 0xa0, 0x47, 0x82, 0xf4, 0x3b, 0xc8,
];

pub(crate) struct IlluminationGuard {
    _restore: RestoreState<FileControl>,
}

pub(crate) fn enable_for_camera(
    device: &v4l::Device,
    device_index: usize,
) -> io::Result<Option<IlluminationGuard>> {
    // Keep the same opened camera available for restoration, even if its
    // original Device has already been dropped. Do not inherit this fd on exec.
    let handle = device.handle();
    let fd = unsafe { libc::fcntl(handle.fd(), libc::F_DUPFD_CLOEXEC, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_fd(fd) };
    if !verified_hardware(&file, device_index)? {
        return Ok(None);
    }

    let mut backend = FileControl {
        file,
        locked: false,
    };
    // Separate CLI and PAM processes must not restore each other's mode while
    // another capture is still running. A busy camera fails without waiting.
    if unsafe { libc::flock(backend.file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } < 0 {
        return Err(io::Error::last_os_error());
    }
    backend.locked = true;
    Ok(enable_verified(backend)?.map(|restore| IlluminationGuard { _restore: restore }))
}

fn verified_hardware(file: &File, device_index: usize) -> io::Result<bool> {
    let metadata = file.metadata()?;
    if !metadata.file_type().is_char_device() {
        return Ok(false);
    }
    let node = Path::new("/sys/class/video4linux").join(format!("video{}", device_index));
    let verify = || -> io::Result<bool> {
        // Bind the sysfs identity to the opened fd, rather than trusting a
        // requested video number that might refer to a different device.
        let device_number = fs::read_to_string(node.join("dev"))?;
        let expected = format!(
            "{}:{}",
            libc::major(metadata.rdev()),
            libc::minor(metadata.rdev())
        );
        if device_number.trim() != expected {
            return Ok(false);
        }
        let interface = fs::canonicalize(node.join("device"))?;
        let Some(usb_device) = interface.parent() else {
            return Ok(false);
        };
        Ok(profile_matches(
            &fs::read_to_string(usb_device.join("idVendor"))?,
            &fs::read_to_string(usb_device.join("idProduct"))?,
            &fs::read_to_string(interface.join("bInterfaceNumber"))?,
            &fs::read(usb_device.join("descriptors"))?,
        ))
    };
    match verify() {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        result => result,
    }
}

fn profile_matches(vendor: &str, product: &str, interface: &str, descriptors: &[u8]) -> bool {
    if vendor.trim() != "0bda" || product.trim() != "636e" || interface.trim() != "02" {
        return false;
    }
    let mut remaining = descriptors;
    let mut matching_interface = false;
    while remaining.len() >= 2 {
        let length = remaining[0] as usize;
        if length < 2 || length > remaining.len() {
            return false;
        }
        let descriptor = &remaining[..length];
        if descriptor[1] == 4 {
            matching_interface =
                length >= 9 && descriptor[2] == 2 && descriptor[5] == 0x0e && descriptor[6] == 1;
        } else if matching_interface
            && descriptor[1] == 0x24
            && length >= 29
            && descriptor[2] == 6
            && descriptor[3] == UNIT
            && descriptor[4..20] == MS_CAMERA_CONTROL_GUID
            && descriptor[20] == 2
            && descriptor[21] == 1
            && descriptor[23] == 4
            && descriptor[24..28] == [0x20, 0x01, 0x00, 0x00]
        {
            return true;
        }
        remaining = &remaining[length..];
    }
    false
}

trait ControlIo {
    fn query(&mut self, request: u8, data: &mut [u8]) -> io::Result<()>;
}

struct FileControl {
    file: File,
    locked: bool,
}

#[repr(C)]
struct UvcXuControlQuery {
    unit: u8,
    selector: u8,
    query: u8,
    size: u16,
    data: *mut u8,
}

impl ControlIo for FileControl {
    fn query(&mut self, request: u8, data: &mut [u8]) -> io::Result<()> {
        let expected_size = match request {
            GET_LEN => 2,
            GET_INFO => 1,
            GET_CUR | GET_MAX | SET_CUR => PAYLOAD_LEN,
            _ => return Err(invalid_control("Unsupported IR control request")),
        };
        if data.len() != expected_size {
            return Err(invalid_control("Unexpected IR control request size"));
        }
        let mut query = UvcXuControlQuery {
            unit: UNIT,
            selector: SELECTOR,
            query: request,
            size: data.len() as u16,
            data: data.as_mut_ptr(),
        };
        let request = libc::_IOWR::<UvcXuControlQuery>(u32::from(b'u'), 0x21);
        if unsafe { libc::ioctl(self.file.as_raw_fd(), request, &mut query) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl Drop for FileControl {
    fn drop(&mut self) {
        if self.locked {
            // Explicitly unlock: the camera's original fd shares this open file
            // description and may outlive our duplicate.
            unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
        }
    }
}

struct RestoreState<B: ControlIo> {
    backend: B,
    previous: [u8; PAYLOAD_LEN],
}

impl<B: ControlIo> Drop for RestoreState<B> {
    fn drop(&mut self) {
        if let Err(error) = self.backend.query(SET_CUR, &mut self.previous) {
            log::warn!("Could not restore IR illumination mode: {}", error);
        }
    }
}

fn enable_verified<B: ControlIo>(mut backend: B) -> io::Result<Option<RestoreState<B>>> {
    let mut length = [0; 2];
    backend.query(GET_LEN, &mut length)?;
    if u16::from_le_bytes(length) as usize != PAYLOAD_LEN {
        return Err(invalid_control("Unexpected IR control payload length"));
    }
    let mut info = [0; 1];
    backend.query(GET_INFO, &mut info)?;
    if info[0] != 3 {
        return Err(invalid_control(
            "IR control is not synchronous and readable/writable",
        ));
    }
    let mut previous = [0; PAYLOAD_LEN];
    backend.query(GET_CUR, &mut previous)?;
    let current_flags = payload_flags(&previous)?;
    if !matches!(current_flags, DISABLED | ALTERNATING) {
        return Err(invalid_control("Unexpected current IR illumination mode"));
    }
    let mut maximum = [0; PAYLOAD_LEN];
    backend.query(GET_MAX, &mut maximum)?;
    let supported = payload_flags(&maximum)?;
    if supported & ALTERNATING == 0
        || supported & !(DISABLED | ALTERNATING) != 0
        || supported & current_flags != current_flags
    {
        return Err(invalid_control(
            "Camera does not advertise the expected IR illumination modes",
        ));
    }
    if current_flags == ALTERNATING {
        return Ok(None);
    }

    // Arm restoration before SET_CUR: a failing write or readback might still
    // leave the device changed.
    let mut guard = RestoreState { backend, previous };
    let mut requested = previous;
    requested[2] = ALTERNATING;
    let mut outgoing = requested;
    guard.backend.query(SET_CUR, &mut outgoing)?;
    let mut actual = [0; PAYLOAD_LEN];
    guard.backend.query(GET_CUR, &mut actual)?;
    if actual != requested {
        return Err(invalid_control("Camera did not apply IR illumination mode"));
    }
    log::debug!("Enabled alternating IR illumination for ASUS 0bda:636e");
    Ok(Some(guard))
}

fn payload_flags(payload: &[u8; PAYLOAD_LEN]) -> io::Result<u8> {
    if payload[0] != 1
        || payload[1] != STREAM_INTERFACE
        || payload[2] & !0x07 != 0
        || payload[3..].iter().any(|&byte| byte != 0)
    {
        return Err(invalid_control(
            "Unexpected IR control entry or reserved flags",
        ));
    }
    Ok(payload[2])
}

fn invalid_control(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    const DISABLED_PAYLOAD: [u8; PAYLOAD_LEN] = [1, 3, 1, 0, 0, 0, 0, 0, 0];
    const ENABLED_PAYLOAD: [u8; PAYLOAD_LEN] = [1, 3, 2, 0, 0, 0, 0, 0, 0];

    #[derive(Clone)]
    struct MockControl {
        state: Rc<RefCell<MockState>>,
    }

    struct MockState {
        length: u16,
        info: u8,
        current: [u8; PAYLOAD_LEN],
        maximum: [u8; PAYLOAD_LEN],
        writes: Vec<Vec<u8>>,
        fail_write: bool,
        fail_readback: bool,
        ignore_write: bool,
    }

    impl MockControl {
        fn new() -> Self {
            Self {
                state: Rc::new(RefCell::new(MockState {
                    length: PAYLOAD_LEN as u16,
                    info: 3,
                    current: DISABLED_PAYLOAD,
                    maximum: [1, 3, 3, 0, 0, 0, 0, 0, 0],
                    writes: Vec::new(),
                    fail_write: false,
                    fail_readback: false,
                    ignore_write: false,
                })),
            }
        }
    }

    impl ControlIo for MockControl {
        fn query(&mut self, request: u8, data: &mut [u8]) -> io::Result<()> {
            let mut state = self.state.borrow_mut();
            match request {
                GET_LEN => data.copy_from_slice(&state.length.to_le_bytes()),
                GET_INFO => data.copy_from_slice(&[state.info]),
                GET_CUR => {
                    if state.fail_readback && !state.writes.is_empty() {
                        return Err(io::Error::other("readback failed"));
                    }
                    data.copy_from_slice(&state.current);
                }
                GET_MAX => data.copy_from_slice(&state.maximum),
                SET_CUR => {
                    state.writes.push(data.to_vec());
                    if !state.ignore_write {
                        state.current.copy_from_slice(data);
                    }
                    if state.fail_write && state.writes.len() == 1 {
                        return Err(io::Error::other("write failed after changing mode"));
                    }
                }
                _ => panic!("Unexpected control query"),
            }
            Ok(())
        }
    }

    #[test]
    fn enabled_mode_is_restored_when_guard_is_dropped() {
        let backend = MockControl::new();
        let guard = enable_verified(backend.clone()).unwrap().unwrap();
        assert_eq!(backend.state.borrow().current, ENABLED_PAYLOAD);
        drop(guard);
        let state = backend.state.borrow();
        assert_eq!(state.current, DISABLED_PAYLOAD);
        assert_eq!(
            state.writes,
            [ENABLED_PAYLOAD.to_vec(), DISABLED_PAYLOAD.to_vec()]
        );
    }

    #[test]
    fn already_enabled_camera_is_not_modified() {
        let backend = MockControl::new();
        backend.state.borrow_mut().current = ENABLED_PAYLOAD;
        assert!(enable_verified(backend.clone()).unwrap().is_none());
        assert!(backend.state.borrow().writes.is_empty());
    }

    #[test]
    fn malformed_or_unsupported_controls_are_never_written() {
        for case in 0..10 {
            let backend = MockControl::new();
            {
                let mut state = backend.state.borrow_mut();
                match case {
                    0 => state.length = 10,
                    1 => state.info = 1,
                    2 => state.current[0] = 2,
                    3 => state.current[1] = 1,
                    4 => state.current[2] = 3,
                    5 => state.current[3] = 1,
                    6 => state.maximum[2] = DISABLED,
                    7 => state.maximum[2] = 7,
                    8 => state.maximum[8] = 1,
                    9 => state.maximum[1] = 1,
                    _ => unreachable!(),
                }
            }
            assert!(enable_verified(backend.clone()).is_err(), "case {}", case);
            assert!(backend.state.borrow().writes.is_empty(), "case {}", case);
        }
    }

    #[test]
    fn failed_write_or_readback_restores_the_previous_mode() {
        for case in 0..3 {
            let backend = MockControl::new();
            {
                let mut state = backend.state.borrow_mut();
                state.fail_write = case == 0;
                state.fail_readback = case == 1;
                state.ignore_write = case == 2;
            }
            assert!(enable_verified(backend.clone()).is_err());
            let state = backend.state.borrow();
            assert_eq!(state.current, DISABLED_PAYLOAD);
            assert_eq!(state.writes.last().unwrap(), &DISABLED_PAYLOAD);
        }
    }

    #[test]
    fn hardware_profile_requires_exact_identity_and_extension_descriptor() {
        let mut descriptors = vec![9, 4, 2, 0, 1, 0x0e, 1, 1, 6];
        descriptors.extend_from_slice(&[
            29, 0x24, 6, 14, 0xdc, 0x95, 0x3f, 0x0f, 0x32, 0x26, 0x4e, 0x4c, 0x92, 0xc9, 0xa0,
            0x47, 0x82, 0xf4, 0x3b, 0xc8, 2, 1, 10, 4, 0x20, 1, 0, 0, 9,
        ]);
        assert!(profile_matches("0bda\n", "636e\n", "02\n", &descriptors));
        for (vendor, product, interface) in [
            ("0000", "636e", "02"),
            ("0bda", "0000", "02"),
            ("0bda", "636e", "00"),
        ] {
            assert!(!profile_matches(vendor, product, interface, &descriptors));
        }
        for offset in [0, 2, 12, 13, 29, 30, 32, 33] {
            let mut wrong = descriptors.clone();
            wrong[offset] ^= 1;
            assert!(
                !profile_matches("0bda", "636e", "02", &wrong),
                "offset {}",
                offset
            );
        }
        descriptors.pop();
        assert!(!profile_matches("0bda", "636e", "02", &descriptors));
    }
}
