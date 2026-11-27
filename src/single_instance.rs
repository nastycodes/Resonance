use std::{ptr::null, thread, time::Duration};

use windows_sys::Win32::{
    Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE},
    System::Threading::{CreateMutexW, ReleaseMutex},
    UI::WindowsAndMessaging::{
        AllowSetForegroundWindow, FindWindowW, GetWindowThreadProcessId, SW_RESTORE,
        SetForegroundWindow, ShowWindow,
    },
};

const MUTEX_NAME: &str = "Local\\Resonance.SingleInstance";
const WINDOW_TITLE: &str = "Resonance";

pub struct InstanceLock {
    handle: HANDLE,
}

pub fn acquire() -> Result<Option<InstanceLock>, String> {
    let name: Vec<u16> = format!("{MUTEX_NAME}\0").encode_utf16().collect();
    let handle = unsafe { CreateMutexW(null(), 1, name.as_ptr()) };
    if handle.is_null() {
        return Err(std::io::Error::last_os_error().to_string());
    }

    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe {
            CloseHandle(handle);
        }
        activate_existing_instance()?;
        return Ok(None);
    }

    Ok(Some(InstanceLock { handle }))
}

fn activate_existing_instance() -> Result<(), String> {
    let title: Vec<u16> = format!("{WINDOW_TITLE}\0").encode_utf16().collect();
    let window = (0..200)
        .find_map(|_| {
            let window = unsafe { FindWindowW(null(), title.as_ptr()) };
            if window.is_null() {
                thread::sleep(Duration::from_millis(50));
                None
            } else {
                Some(window)
            }
        })
        .ok_or_else(|| "The running Resonance window could not be found".to_string())?;

    let mut process_id = 0;
    unsafe {
        GetWindowThreadProcessId(window, &mut process_id);
        if process_id != 0 {
            AllowSetForegroundWindow(process_id);
        }
        ShowWindow(window, SW_RESTORE);
        if SetForegroundWindow(window) == 0 {
            return Err("Windows did not allow the Resonance window to be activated".into());
        }
    }

    Ok(())
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        unsafe {
            ReleaseMutex(self.handle);
            CloseHandle(self.handle);
        }
    }
}
