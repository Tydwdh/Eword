//! Chinese/English input-language switching.
//!
//! A terminal application cannot intercept IME composition: the terminal
//! receives composed text and forwards it through the console/ConPTY. What we
//! can do is ask the host window to switch its Windows input language. The
//! active terminal window then behaves as if the user pressed the language
//! hotkey. This is best-effort and is disabled automatically when the layouts
//! are not installed or the host does not expose a foreground window.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImeLang {
    English,
    Chinese,
}

impl ImeLang {
    #[allow(dead_code)]
    pub fn label(self) -> &'static str {
        match self {
            Self::English => "EN",
            Self::Chinese => "中",
        }
    }
}

#[cfg(windows)]
mod platform {
    use std::mem::size_of;

    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetKeyboardLayout, GetKeyboardLayoutList, HKL,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GUITHREADINFO, GetClassNameW, GetForegroundWindow, GetGUIThreadInfo,
        GetWindowThreadProcessId, PostMessageW,
    };

    use super::ImeLang;

    const WM_INPUTLANGCHANGEREQUEST: u32 = 0x0050;
    const LANG_ENGLISH: u16 = 0x09;
    const LANG_CHINESE: u16 = 0x04;

    pub struct ImeController {
        enabled: bool,
        active: bool,
        host: HWND,
        host_pid: u32,
        host_class: String,
        original: Option<HKL>,
        english: Option<HKL>,
        chinese: Option<HKL>,
        last: Option<ImeLang>,
        reason: Option<String>,
    }

    impl ImeController {
        pub fn new(enabled: bool) -> Self {
            Self {
                enabled,
                active: false,
                host: std::ptr::null_mut(),
                host_pid: 0,
                host_class: String::new(),
                original: None,
                english: None,
                chinese: None,
                last: None,
                reason: None,
            }
        }

        pub fn activate(&mut self) {
            if self.active {
                return;
            }
            let host = unsafe { GetForegroundWindow() };
            if host.is_null() {
                self.reason = Some("no foreground window".to_string());
                return;
            }
            let mut pid = 0u32;
            let tid = unsafe { GetWindowThreadProcessId(host, &mut pid) };
            if tid == 0 {
                self.reason = Some("no foreground thread".to_string());
                return;
            }

            let layouts = list_layouts();
            self.english = layouts
                .iter()
                .copied()
                .find(|hkl| primary_language(*hkl) == LANG_ENGLISH);
            self.chinese = layouts
                .iter()
                .copied()
                .find(|hkl| primary_language(*hkl) == LANG_CHINESE);

            self.host = host;
            self.host_pid = pid;
            self.host_class = window_class(host);
            self.original = Some(unsafe { GetKeyboardLayout(tid) });
            self.last = self.current_lang();

            if self.english.is_none() || self.chinese.is_none() {
                self.reason = Some("need both en-US and zh-CN keyboard layouts".to_string());
                self.active = false;
                return;
            }

            self.reason = None;
            self.active = true;
        }

        pub fn deactivate(&mut self) {
            if !self.active {
                return;
            }
            if let Some(original) = self.original
                && !self.host.is_null()
            {
                let _ = post_language(self.host, original);
            }
            self.active = false;
        }

        pub fn set_enabled(&mut self, enabled: bool) {
            self.enabled = enabled;
            if enabled {
                self.activate();
            }
            // Turning auto off leaves the field in its current language; the
            // original layout is still restored when the TUI exits.
        }

        pub fn auto(&mut self, lang: ImeLang) {
            if self.enabled {
                self.switch(lang);
            }
        }

        pub fn force(&mut self, lang: ImeLang) {
            if !self.active {
                self.activate();
            }
            self.switch(lang);
        }

        pub fn toggle(&mut self) {
            if !self.active {
                self.activate();
            }
            if !self.active {
                return;
            }
            let current = self
                .current_lang()
                .or(self.last)
                .unwrap_or(ImeLang::English);
            self.force(match current {
                ImeLang::English => ImeLang::Chinese,
                ImeLang::Chinese => ImeLang::English,
            });
        }

        fn switch(&mut self, lang: ImeLang) {
            if !self.active {
                return;
            }
            let Some(target) = (match lang {
                ImeLang::English => self.english,
                ImeLang::Chinese => self.chinese,
            }) else {
                return;
            };
            if self.current_hkl() == Some(target) {
                self.last = Some(lang);
                return;
            }

            let foreground = unsafe { GetForegroundWindow() };
            if foreground.is_null() {
                return;
            }
            let mut pid = 0u32;
            let tid = unsafe { GetWindowThreadProcessId(foreground, &mut pid) };
            if tid == 0 || pid != self.host_pid {
                return;
            }
            if post_language(foreground, target) {
                self.last = Some(lang);
            }
        }

        fn current_hkl(&self) -> Option<HKL> {
            let foreground = unsafe { GetForegroundWindow() };
            if foreground.is_null() {
                return None;
            }
            let tid = unsafe { GetWindowThreadProcessId(foreground, std::ptr::null_mut()) };
            if tid == 0 {
                None
            } else {
                Some(unsafe { GetKeyboardLayout(tid) })
            }
        }

        fn current_lang(&self) -> Option<ImeLang> {
            let hkl = self.current_hkl()?;
            match primary_language(hkl) {
                LANG_ENGLISH => Some(ImeLang::English),
                LANG_CHINESE => Some(ImeLang::Chinese),
                _ => None,
            }
        }

        pub fn status(&self) -> String {
            if !self.enabled {
                return format!(
                    "ime auto off · F2 switches · original restored on exit{}",
                    self.reason
                        .as_ref()
                        .map(|reason| format!(" · {reason}"))
                        .unwrap_or_default()
                );
            }
            if !self.active {
                return format!(
                    "ime auto unavailable{}",
                    self.reason
                        .as_ref()
                        .map(|reason| format!(" · {reason}"))
                        .unwrap_or_default()
                );
            }
            let en = self
                .english
                .map(|hkl| format!("{:04X}", language_id(hkl)))
                .unwrap_or_else(|| "—".to_string());
            let zh = self
                .chinese
                .map(|hkl| format!("{:04X}", language_id(hkl)))
                .unwrap_or_else(|| "—".to_string());
            format!(
                "ime auto · host {} · en {} · zh {} · last {}",
                truncate(&self.host_class, 24),
                en,
                zh,
                self.last.map(ImeLang::label).unwrap_or("—")
            )
        }
    }

    impl Drop for ImeController {
        fn drop(&mut self) {
            self.deactivate();
        }
    }

    fn list_layouts() -> Vec<HKL> {
        let count = unsafe { GetKeyboardLayoutList(0, std::ptr::null_mut()) };
        if count <= 0 {
            return Vec::new();
        }
        let mut layouts = vec![std::ptr::null_mut(); count as usize];
        let loaded = unsafe { GetKeyboardLayoutList(count, layouts.as_mut_ptr()) };
        if loaded > 0 {
            layouts.truncate(loaded as usize);
            layouts
        } else {
            Vec::new()
        }
    }

    fn primary_language(hkl: HKL) -> u16 {
        (language_id(hkl) & 0x03FF) as u16
    }

    fn language_id(hkl: HKL) -> u32 {
        (hkl as usize as u32) & 0xFFFF
    }

    fn post_language(host: HWND, hkl: HKL) -> bool {
        let target = focused_window(host);
        unsafe { PostMessageW(target, WM_INPUTLANGCHANGEREQUEST, 0, hkl as isize) != 0 }
    }

    fn focused_window(host: HWND) -> HWND {
        let tid = unsafe { GetWindowThreadProcessId(host, std::ptr::null_mut()) };
        if tid == 0 {
            return host;
        }
        let mut info: GUITHREADINFO = unsafe { std::mem::zeroed() };
        info.cbSize = size_of::<GUITHREADINFO>() as u32;
        if unsafe { GetGUIThreadInfo(tid, &mut info) } != 0 && !info.hwndFocus.is_null() {
            info.hwndFocus
        } else {
            host
        }
    }

    fn window_class(hwnd: HWND) -> String {
        let mut buffer = [0u16; 256];
        let length = unsafe { GetClassNameW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };
        if length > 0 {
            String::from_utf16_lossy(&buffer[..length as usize])
        } else {
            "unknown".to_string()
        }
    }

    fn truncate(value: &str, max: usize) -> String {
        if value.chars().count() <= max {
            value.to_string()
        } else {
            value.chars().take(max).collect::<String>() + "…"
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use super::ImeLang;

    pub struct ImeController {
        enabled: bool,
    }

    impl ImeController {
        pub fn new(enabled: bool) -> Self {
            Self { enabled }
        }
        pub fn activate(&mut self) {}
        pub fn deactivate(&mut self) {}
        pub fn set_enabled(&mut self, enabled: bool) {
            self.enabled = enabled;
        }
        pub fn auto(&mut self, _lang: ImeLang) {
            let _ = self.enabled;
        }
        pub fn force(&mut self, _lang: ImeLang) {}
        pub fn toggle(&mut self) {}
        pub fn status(&self) -> String {
            if self.enabled {
                "ime switching is only available on Windows".to_string()
            } else {
                "ime auto off".to_string()
            }
        }
    }
}

pub use platform::ImeController;
