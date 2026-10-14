//! One archive writer at a time, across PROCESSES - the same named mutex the
//! Python backend takes (kspkg_write.archive_lock). Held for a whole
//! read-edit-write of an archive: two writers corrupt it, and two
//! interleaved table edits lose a row. Recursive for its owning thread.

pub struct Guard {
    #[cfg(windows)]
    h: windows_sys::Win32::Foundation::HANDLE,
}

pub fn lock() -> Result<Guard, String> {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{CreateMutexW, WaitForSingleObject};
        let name: Vec<u16> = "Local\\ACECM_archive_write".encode_utf16().chain(Some(0)).collect();
        let h = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        if h.is_null() {
            return Err("could not create the archive lock".into());
        }
        let r = WaitForSingleObject(h, 600_000);
        if r != 0 && r != 0x80 {
            CloseHandle(h);
            return Err("another ACECM process is writing the archive".into());
        }
        Ok(Guard { h })
    }
    #[cfg(not(windows))]
    Ok(Guard {})
}

impl Drop for Guard {
    fn drop(&mut self) {
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::System::Threading::ReleaseMutex(self.h);
            windows_sys::Win32::Foundation::CloseHandle(self.h);
        }
    }
}
