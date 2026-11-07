//! Windows console fixes for IME support.
//!
//! crossterm's raw mode can leave the console without `ENABLE_WINDOW_INPUT`.
//! Chinese/Japanese/Korean IME composition then stops committing text. This
//! guard restores the flag while the TUI runs and restores the previous console
//! mode on exit.

#[cfg(windows)]
pub struct ConsoleModeGuard {
    handle: windows_sys::Win32::Foundation::HANDLE,
    original: u32,
}

#[cfg(windows)]
impl ConsoleModeGuard {
    pub fn ensure_ime() -> Self {
        use windows_sys::Win32::System::Console::{
            ENABLE_WINDOW_INPUT, GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE, SetConsoleMode,
        };
        const ENABLE_QUICK_EDIT_MODE: u32 = 0x0040;
        const ENABLE_EXTENDED_FLAGS: u32 = 0x0080;

        unsafe {
            let handle = GetStdHandle(STD_INPUT_HANDLE);
            if handle.is_null() {
                return Self {
                    handle,
                    original: 0,
                };
            }
            let mut mode = 0u32;
            if GetConsoleMode(handle, &mut mode) == 0 {
                return Self {
                    handle: std::ptr::null_mut(),
                    original: 0,
                };
            }
            let desired =
                (mode | ENABLE_WINDOW_INPUT | ENABLE_EXTENDED_FLAGS) & !ENABLE_QUICK_EDIT_MODE;
            if SetConsoleMode(handle, desired) == 0 {
                let _ = SetConsoleMode(handle, mode | ENABLE_WINDOW_INPUT);
            }
            Self {
                handle,
                original: mode,
            }
        }
    }
}

#[cfg(windows)]
impl Drop for ConsoleModeGuard {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                let _ =
                    windows_sys::Win32::System::Console::SetConsoleMode(self.handle, self.original);
            }
        }
    }
}

#[cfg(not(windows))]
pub struct ConsoleModeGuard;

#[cfg(not(windows))]
impl ConsoleModeGuard {
    pub fn ensure_ime() -> Self {
        Self
    }
}
