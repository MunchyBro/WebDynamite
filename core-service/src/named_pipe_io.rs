//! Bounded overlapped byte I/O for handles created with FILE_FLAG_OVERLAPPED.
//!
//! Each Read/Write owns its event and OVERLAPPED until completion or confirmed
//! cancellation. Do not use this adapter with synchronous pipe handles.
#[cfg(windows)]
pub struct OverlappedPipeIo {
    handle: windows_sys::Win32::Foundation::HANDLE,
    timeout_ms: u32,
}

#[cfg(windows)]
impl OverlappedPipeIo {
    pub fn new(handle: windows_sys::Win32::Foundation::HANDLE, timeout_ms: u32) -> Result<Self, String> {
        if handle.is_null() {
            return Err("A valid overlapped pipe handle is required.".into());
        }
        if timeout_ms == 0 {
            return Err("Overlapped pipe I/O timeout must be greater than zero.".into());
        }
        Ok(Self { handle, timeout_ms })
    }

    fn transfer(&self, buffer: *mut u8, length: u32, write: bool) -> std::io::Result<usize> {
        use std::{io, ptr};
        use windows_sys::Win32::{
            Foundation::{CloseHandle, GetLastError, ERROR_IO_PENDING, WAIT_OBJECT_0, WAIT_TIMEOUT},
            Storage::FileSystem::{ReadFile, WriteFile},
            System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED},
            System::Threading::{CreateEventW, WaitForSingleObject},
        };

        let event = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
        if event.is_null() {
            return Err(io::Error::last_os_error());
        }

        let result = (|| {
            let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
            overlapped.hEvent = event;
            let started = unsafe {
                if write {
                    WriteFile(self.handle, buffer.cast_const().cast(), length, ptr::null_mut(), &mut overlapped)
                } else {
                    ReadFile(self.handle, buffer.cast(), length, ptr::null_mut(), &mut overlapped)
                }
            };
            if started == 0 {
                let error = unsafe { GetLastError() };
                if error != ERROR_IO_PENDING {
                    return Err(io::Error::from_raw_os_error(error as i32));
                }
            }

            if started == 0 {
                let wait = unsafe { WaitForSingleObject(event, self.timeout_ms) };
                if wait == WAIT_TIMEOUT {
                    unsafe { CancelIoEx(self.handle, &overlapped); }
                    let mut transferred = 0u32;
                    let completed = unsafe { GetOverlappedResult(self.handle, &overlapped, &mut transferred, 1) };
                    if completed != 0 {
                        return Ok(transferred as usize);
                    }
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "Overlapped pipe I/O timed out."));
                }
                if wait != WAIT_OBJECT_0 {
                    let wait_error = unsafe { GetLastError() };
                    unsafe { CancelIoEx(self.handle, &overlapped); }
                    let mut transferred = 0u32;
                    unsafe { GetOverlappedResult(self.handle, &overlapped, &mut transferred, 1); }
                    return Err(io::Error::from_raw_os_error(wait_error as i32));
                }
            }

            let mut transferred = 0u32;
            if unsafe { GetOverlappedResult(self.handle, &overlapped, &mut transferred, 0) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(transferred as usize)
        })();

        unsafe { CloseHandle(event); }
        result
    }
}

#[cfg(windows)]
impl std::io::Read for OverlappedPipeIo {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if buffer.is_empty() { return Ok(0); }
        self.transfer(buffer.as_mut_ptr(), buffer.len().min(u32::MAX as usize) as u32, false)
    }
}

#[cfg(windows)]
impl std::io::Write for OverlappedPipeIo {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        if buffer.is_empty() { return Ok(0); }
        self.transfer(buffer.as_ptr() as *mut u8, buffer.len().min(u32::MAX as usize) as u32, true)
    }

    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

#[cfg(not(windows))]
pub struct OverlappedPipeIo;

#[cfg(not(windows))]
impl OverlappedPipeIo {
    pub fn new(_handle: isize, _timeout_ms: u32) -> Result<Self, String> {
        Err("Overlapped pipe I/O is available only on Windows.".into())
    }
}

#[cfg(test)]
mod tests {
    #[cfg(not(windows))]
    #[test]
    fn non_windows_reports_unsupported_platform() {
        assert!(super::OverlappedPipeIo::new(0, 5_000).unwrap_err().contains("only on Windows"));
    }
}
