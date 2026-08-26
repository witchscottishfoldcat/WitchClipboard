#[cfg(windows)]
mod imp {
    use std::{
        collections::HashMap,
        ffi::c_void,
        path::Path,
        ptr::{null, null_mut},
        sync::{mpsc, Mutex, OnceLock},
        thread,
        time::Duration,
    };

    use windows_sys::Win32::{
        Foundation::{CloseHandle, GlobalFree, HWND, LPARAM, LRESULT, WPARAM},
        System::{
            DataExchange::{
                AddClipboardFormatListener, CloseClipboard, EmptyClipboard, GetClipboardData,
                GetClipboardSequenceNumber, IsClipboardFormatAvailable, OpenClipboard,
                RegisterClipboardFormatW, RemoveClipboardFormatListener, SetClipboardData,
            },
            LibraryLoader::GetModuleHandleW,
            Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE},
            Registry::{
                RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegSetValueExW, HKEY_CURRENT_USER,
                KEY_SET_VALUE, REG_SZ,
            },
            Threading::{
                OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
            },
        },
        UI::{
            HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi},
            Input::KeyboardAndMouse::{
                keybd_event, GetAsyncKeyState, KEYEVENTF_KEYUP, VK_CONTROL, VK_LWIN, VK_MENU,
                VK_RWIN, VK_SHIFT,
            },
            Shell::DragQueryFileW,
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
                GetForegroundWindow, GetMessageW, GetWindowThreadProcessId, MessageBoxW,
                PrivateExtractIconsW, RegisterClassW, SendMessageW, SetForegroundWindow,
                TranslateMessage, HICON, HWND_MESSAGE, ICON_BIG, ICON_SMALL, MB_ICONERROR, MB_OK,
                MSG, SM_CXICON, SM_CXSMICON, WM_CLIPBOARDUPDATE, WM_SETICON, WNDCLASSW,
            },
        },
    };

    const CF_HDROP: u32 = 15;
    const VK_V: u8 = 0x56;
    const DROPFILES_HEADER_BYTES: usize = 20;
    const SENSITIVE_FORMATS: &[&str] = &[
        "ExcludeClipboardContentFromMonitorProcessing",
        "CanIncludeInClipboardHistory",
        "Clipboard Viewer Ignore",
    ];
    static CLIPBOARD_EVENTS: OnceLock<mpsc::Sender<()>> = OnceLock::new();
    // Store the extracted HICON handles for the process lifetime. Windows does not copy handles
    // passed through WM_SETICON, so destroying them while a window is alive would leave it with
    // dangling icons. The OS reclaims both handles when the process exits.
    static WINDOW_ICONS: OnceLock<Mutex<HashMap<(i32, i32), (isize, isize)>>> = OnceLock::new();

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }

    pub fn show_fatal_error(message: &str) {
        let title = wide("Witch Clipboard 启动失败");
        let body = wide(message);
        unsafe {
            MessageBoxW(
                null_mut(),
                body.as_ptr(),
                title.as_ptr(),
                MB_OK | MB_ICONERROR,
            );
        }
    }

    pub fn set_window_icons(window: &tauri::WebviewWindow) -> Result<(), String> {
        let hwnd = window.hwnd().map_err(|error| error.to_string())?.0 as HWND;
        let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
        let large_size = unsafe { GetSystemMetricsForDpi(SM_CXICON, dpi) }.max(32);
        let small_size = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, dpi) }.max(16);
        let cache = WINDOW_ICONS.get_or_init(|| Mutex::new(HashMap::new()));
        let mut cache = cache
            .lock()
            .map_err(|_| "window icon cache lock poisoned".to_string())?;
        let (large, small) = *cache.entry((large_size, small_size)).or_insert_with(|| {
            let Ok(executable) = std::env::current_exe() else {
                return (0, 0);
            };
            let path = wide(&executable.to_string_lossy());
            let mut large: HICON = null_mut();
            let mut small: HICON = null_mut();
            let mut large_id = 0;
            let mut small_id = 0;
            // Extract exact per-monitor DPI sizes. In particular, Windows at 150% needs a
            // native 48 px taskbar icon and 24 px small icon; scaling the 16 px ICO frame up to
            // 24 px is visibly blurry.
            let large_count = unsafe {
                PrivateExtractIconsW(
                    path.as_ptr(),
                    0,
                    large_size,
                    large_size,
                    &mut large,
                    &mut large_id,
                    1,
                    0,
                )
            };
            let small_count = unsafe {
                PrivateExtractIconsW(
                    path.as_ptr(),
                    0,
                    small_size,
                    small_size,
                    &mut small,
                    &mut small_id,
                    1,
                    0,
                )
            };
            if large_count == 0 || small_count == 0 {
                (0, 0)
            } else {
                (large as isize, small as isize)
            }
        });
        if large == 0 || small == 0 {
            return Err("failed to extract embedded window icons".to_string());
        }

        unsafe {
            SendMessageW(hwnd, WM_SETICON, ICON_BIG as WPARAM, large as LPARAM);
            SendMessageW(hwnd, WM_SETICON, ICON_SMALL as WPARAM, small as LPARAM);
        }
        Ok(())
    }

    unsafe extern "system" fn clipboard_window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if message == WM_CLIPBOARDUPDATE {
            if let Some(sender) = CLIPBOARD_EVENTS.get() {
                let _ = sender.send(());
            }
            return 0;
        }
        unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
    }

    pub fn start_clipboard_notifications() -> Result<mpsc::Receiver<()>, String> {
        let (event_sender, event_receiver) = mpsc::channel();
        CLIPBOARD_EVENTS
            .set(event_sender)
            .map_err(|_| "clipboard listener already started".to_string())?;

        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        thread::spawn(move || unsafe {
            let class_name = wide("WitchClipboardTauriListener");
            let instance = GetModuleHandleW(null());
            let mut class: WNDCLASSW = std::mem::zeroed();
            class.lpfnWndProc = Some(clipboard_window_proc);
            class.hInstance = instance;
            class.lpszClassName = class_name.as_ptr();

            if RegisterClassW(&class) == 0 {
                let _ = ready_sender.send(Err("RegisterClassW failed".to_string()));
                return;
            }

            let hwnd = CreateWindowExW(
                0,
                class_name.as_ptr(),
                class_name.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                null_mut(),
                instance,
                null(),
            );
            if hwnd.is_null() {
                let _ = ready_sender.send(Err("CreateWindowExW failed".to_string()));
                return;
            }
            if AddClipboardFormatListener(hwnd) == 0 {
                DestroyWindow(hwnd);
                let _ = ready_sender.send(Err("AddClipboardFormatListener failed".to_string()));
                return;
            }

            let _ = ready_sender.send(Ok(()));
            let mut message: MSG = std::mem::zeroed();
            while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            RemoveClipboardFormatListener(hwnd);
            DestroyWindow(hwnd);
        });

        match ready_receiver.recv_timeout(Duration::from_secs(2)) {
            Ok(Ok(())) => Ok(event_receiver),
            Ok(Err(error)) => Err(error),
            Err(error) => Err(format!("clipboard listener startup timed out: {error}")),
        }
    }

    pub fn clipboard_sequence() -> u32 {
        unsafe { GetClipboardSequenceNumber() }
    }

    pub fn foreground_window() -> Option<isize> {
        let hwnd = unsafe { GetForegroundWindow() };
        (!hwnd.is_null()).then_some(hwnd as isize)
    }

    pub fn window_process_id(hwnd: isize) -> Option<u32> {
        if hwnd == 0 {
            return None;
        }
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(hwnd as HWND, &mut pid) };
        (pid != 0).then_some(pid)
    }

    pub fn foreground_exe() -> Option<String> {
        let hwnd = foreground_window()?;
        let pid = window_process_id(hwnd)?;
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if process.is_null() {
            return None;
        }

        let mut buffer = vec![0u16; 1024];
        let mut length = buffer.len() as u32;
        let ok = unsafe {
            QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut length) != 0
        };
        unsafe { CloseHandle(process) };
        if !ok {
            return None;
        }

        let full_path = String::from_utf16_lossy(&buffer[..length as usize]);
        Path::new(&full_path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
    }

    pub fn has_sensitive_marker() -> bool {
        SENSITIVE_FORMATS.iter().any(|name| {
            let wide_name = wide(name);
            let format = unsafe { RegisterClipboardFormatW(wide_name.as_ptr()) };
            format != 0 && unsafe { IsClipboardFormatAvailable(format) } != 0
        })
    }

    pub fn set_auto_launch(enabled: bool) -> bool {
        let subkey = wide("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
        let value_name = wide("Witch Clipboard");
        let mut key = std::ptr::null_mut();
        if unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                0,
                KEY_SET_VALUE,
                &mut key,
            )
        } != 0
        {
            return false;
        }
        let ok = if enabled {
            let Ok(exe) = std::env::current_exe() else {
                unsafe { RegCloseKey(key) };
                return false;
            };
            let command = wide(&format!("\"{}\" --hidden", exe.display()));
            unsafe {
                RegSetValueExW(
                    key,
                    value_name.as_ptr(),
                    0,
                    REG_SZ,
                    command.as_ptr() as *const u8,
                    (command.len() * 2) as u32,
                ) == 0
            }
        } else {
            let result = unsafe { RegDeleteValueW(key, value_name.as_ptr()) };
            result == 0 || result == 2
        };
        unsafe { RegCloseKey(key) };
        ok
    }

    pub fn restore_and_paste(target: isize) -> Result<(), &'static str> {
        if target == 0 {
            return Err("no-target");
        }
        thread::sleep(Duration::from_millis(50));
        let mut focused = unsafe { SetForegroundWindow(target as HWND) != 0 };
        if !focused {
            thread::sleep(Duration::from_millis(80));
            focused = unsafe { SetForegroundWindow(target as HWND) != 0 };
        }
        if !focused {
            return Err("focus-failed");
        }

        thread::sleep(Duration::from_millis(60));
        unsafe {
            for key in [VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN, VK_CONTROL] {
                if (GetAsyncKeyState(key as i32) & i16::MIN) != 0 {
                    keybd_event(key as u8, 0, KEYEVENTF_KEYUP, 0);
                }
            }
            keybd_event(VK_CONTROL as u8, 0, 0, 0);
            keybd_event(VK_V, 0, 0, 0);
            keybd_event(VK_V, 0, KEYEVENTF_KEYUP, 0);
            keybd_event(VK_CONTROL as u8, 0, KEYEVENTF_KEYUP, 0);
        }
        Ok(())
    }

    struct ClipboardGuard;

    impl ClipboardGuard {
        fn open() -> Option<Self> {
            (unsafe { OpenClipboard(null_mut()) } != 0).then_some(Self)
        }
    }

    impl Drop for ClipboardGuard {
        fn drop(&mut self) {
            unsafe { CloseClipboard() };
        }
    }

    pub fn read_clipboard_files() -> Option<Vec<String>> {
        if unsafe { IsClipboardFormatAvailable(CF_HDROP) } == 0 {
            return None;
        }
        let _guard = ClipboardGuard::open()?;
        let handle = unsafe { GetClipboardData(CF_HDROP) };
        if handle.is_null() {
            return None;
        }

        let count = unsafe { DragQueryFileW(handle as _, u32::MAX, null_mut(), 0) };
        if count == 0 {
            return None;
        }

        let mut paths = Vec::with_capacity(count as usize);
        for index in 0..count {
            let length = unsafe { DragQueryFileW(handle as _, index, null_mut(), 0) };
            if length == 0 {
                continue;
            }
            let mut buffer = vec![0u16; length as usize + 1];
            let written = unsafe {
                DragQueryFileW(handle as _, index, buffer.as_mut_ptr(), buffer.len() as u32)
            };
            if written > 0 {
                paths.push(String::from_utf16_lossy(&buffer[..written as usize]));
            }
        }
        (!paths.is_empty()).then_some(paths)
    }

    pub fn write_clipboard_files(paths: &[String]) -> bool {
        if paths.is_empty() {
            return false;
        }

        let payload = dropfiles_payload(paths);
        let bytes = payload.len();
        let memory = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes) };
        if memory.is_null() {
            return false;
        }
        let locked = unsafe { GlobalLock(memory) };
        if locked.is_null() {
            unsafe { GlobalFree(memory) };
            return false;
        }

        unsafe {
            std::ptr::copy_nonoverlapping(payload.as_ptr(), locked as *mut u8, bytes);
            GlobalUnlock(memory);
        }

        let Some(_guard) = ClipboardGuard::open() else {
            unsafe { GlobalFree(memory) };
            return false;
        };
        if unsafe { EmptyClipboard() } == 0 {
            unsafe { GlobalFree(memory) };
            return false;
        }
        if unsafe { SetClipboardData(CF_HDROP, memory as *mut c_void) }.is_null() {
            unsafe { GlobalFree(memory) };
            return false;
        }

        // The system owns the allocation after SetClipboardData succeeds.
        true
    }

    fn dropfiles_payload(paths: &[String]) -> Vec<u8> {
        let mut path_list = Vec::<u16>::new();
        for path in paths {
            path_list.extend(path.encode_utf16());
            path_list.push(0);
        }
        path_list.push(0);

        let mut payload = vec![0u8; DROPFILES_HEADER_BYTES + path_list.len() * 2];
        payload[0..4].copy_from_slice(&(DROPFILES_HEADER_BYTES as u32).to_le_bytes());
        payload[16..20].copy_from_slice(&1u32.to_le_bytes());
        for (index, unit) in path_list.iter().enumerate() {
            let offset = DROPFILES_HEADER_BYTES + index * 2;
            payload[offset..offset + 2].copy_from_slice(&unit.to_le_bytes());
        }
        payload
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn dropfiles_payload_has_wide_flag_and_double_null_terminator() {
            let payload = dropfiles_payload(&[
                "C:\\alpha.txt".to_string(),
                "D:\\中文\\beta.png".to_string(),
            ]);
            assert_eq!(u32::from_le_bytes(payload[0..4].try_into().unwrap()), 20);
            assert_eq!(u32::from_le_bytes(payload[16..20].try_into().unwrap()), 1);
            assert_eq!(&payload[payload.len() - 4..], &[0, 0, 0, 0]);
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use std::sync::mpsc;

    pub fn start_clipboard_notifications() -> Result<mpsc::Receiver<()>, String> {
        Err("native clipboard notifications are only available on Windows".to_string())
    }
    pub fn clipboard_sequence() -> u32 {
        0
    }
    pub fn foreground_window() -> Option<isize> {
        None
    }
    pub fn window_process_id(_hwnd: isize) -> Option<u32> {
        None
    }
    pub fn foreground_exe() -> Option<String> {
        None
    }
    pub fn has_sensitive_marker() -> bool {
        false
    }
    pub fn set_auto_launch(_enabled: bool) -> bool {
        false
    }
    pub fn restore_and_paste(_target: isize) -> Result<(), &'static str> {
        Err("no-native")
    }
    pub fn read_clipboard_files() -> Option<Vec<String>> {
        None
    }
    pub fn write_clipboard_files(_paths: &[String]) -> bool {
        false
    }
    pub fn show_fatal_error(message: &str) {
        eprintln!("Witch Clipboard startup failed: {message}");
    }
    pub fn set_window_icons(_window: &tauri::WebviewWindow) -> Result<(), String> {
        Ok(())
    }
}

pub use imp::*;
