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
        Foundation::{CloseHandle, GlobalFree, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY},
        System::{
            DataExchange::{
                AddClipboardFormatListener, CloseClipboard, EmptyClipboard, GetClipboardData,
                GetClipboardOwner, GetClipboardSequenceNumber, IsClipboardFormatAvailable,
                OpenClipboard, RegisterClipboardFormatW, RemoveClipboardFormatListener,
                SetClipboardData,
            },
            LibraryLoader::GetModuleHandleW,
            Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE},
            Registry::{
                RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegSetValueExW, HKEY_CURRENT_USER,
                KEY_SET_VALUE, REG_SZ,
            },
            Threading::{
                AttachThreadInput, GetCurrentThreadId, OpenProcess, OpenProcessToken,
                QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
            },
        },
        UI::{
            HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi},
            Input::KeyboardAndMouse::{
                GetAsyncKeyState, MapVirtualKeyW, SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT,
                KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MAPVK_VK_TO_VSC, VK_CONTROL, VK_ESCAPE,
                VK_LBUTTON, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
            },
            Shell::{DragQueryFileW, ShellExecuteW},
            WindowsAndMessaging::{
                BringWindowToTop, CallNextHookEx, CreateWindowExW, DefWindowProcW, DestroyWindow,
                DispatchMessageW, GetClassNameW, GetCursorPos, GetForegroundWindow,
                GetGUIThreadInfo, GetMessageW, GetWindowRect, GetWindowThreadProcessId, IsIconic,
                IsWindow, IsWindowVisible, MessageBoxW, PostMessageW, PrivateExtractIconsW,
                RegisterClassW, SendMessageTimeoutW, SendMessageW, SetForegroundWindow,
                SetWindowPos, SetWindowsHookExW, ShowWindow, SystemParametersInfoW,
                TranslateMessage, UnhookWindowsHookEx, GUITHREADINFO, HICON, HWND_MESSAGE,
                HWND_TOPMOST, ICON_BIG, ICON_SMALL, MB_ICONERROR, MB_OK, MSG, MSLLHOOKSTRUCT,
                SMTO_ABORTIFHUNG, SM_CXICON, SM_CXSMICON, SPI_GETFOREGROUNDLOCKTIMEOUT,
                SPI_SETFOREGROUNDLOCKTIMEOUT, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
                SWP_SHOWWINDOW, SW_HIDE, SW_RESTORE, SW_SHOWNORMAL, WH_MOUSE_LL, WM_CANCELMODE,
                WM_CLIPBOARDUPDATE, WM_LBUTTONDOWN, WM_QUIT, WM_RBUTTONDOWN, WM_SETICON, WNDCLASSW,
            },
        },
    };

    const CF_UNICODETEXT: u32 = 13;
    const CF_DIB: u32 = 8;
    const CF_DIBV5: u32 = 17;
    const CF_HDROP: u32 = 15;
    const VK_V: u8 = 0x56;
    const DROPFILES_HEADER_BYTES: usize = 20;
    // The clipboard is a system-wide singleton: other processes (or sibling listeners in this
    // one) routinely hold it for a few milliseconds. One OpenClipboard call fails often enough
    // under contention that every native read/write path needs a short retry.
    const OPEN_CLIPBOARD_ATTEMPTS: usize = 5;
    const OPEN_CLIPBOARD_RETRY_DELAY: Duration = Duration::from_millis(5);
    // Presence of either format is an unconditional request to stay out of history.
    const SENSITIVE_PRESENCE_FORMATS: &[&str] = &[
        "ExcludeClipboardContentFromMonitorProcessing",
        "Clipboard Viewer Ignore",
    ];
    // Value-based flag: a DWORD of 0 opts out of history, any non-zero value is an explicit
    // opt-in. Presence alone cannot decide — the payload must be read.
    const HISTORY_FLAG_FORMAT: &str = "CanIncludeInClipboardHistory";
    // The clipboard listener must survive session events (remote desktop reconnects,
    // clipboard service restarts) that can silently kill event delivery. It is tracked in
    // a replaceable slot so the watchdog can tear it down and have it rebuilt.
    struct ListenerHandle {
        sender: mpsc::Sender<()>,
        window: isize,
    }
    static CLIPBOARD_LISTENER: Mutex<Option<ListenerHandle>> = Mutex::new(None);
    // Store the extracted HICON handles for the process lifetime. Windows does not copy handles
    // passed through WM_SETICON, so destroying them while a window is alive would leave it with
    // dangling icons. The OS reclaims both handles when the process exits.
    static WINDOW_ICONS: OnceLock<Mutex<HashMap<(i32, i32), (isize, isize)>>> = OnceLock::new();
    static MOUSE_CLICKS: Mutex<Option<mpsc::Sender<(i32, i32, i64)>>> = Mutex::new(None);

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

    // ShellExecuteW launches whatever handler the registry maps for the target, so a URL taken
    // from the renderer must be pinned to schemes that can only resolve to a browser or mail app.
    fn is_allowed_external_url(url: &str) -> bool {
        let lowered = url.to_ascii_lowercase();
        lowered.starts_with("https://")
            || lowered.starts_with("http://")
            || lowered.starts_with("mailto:")
    }

    pub fn open_url(url: &str) -> Result<(), String> {
        if !is_allowed_external_url(url) {
            return Err(format!("unsupported external url scheme: {url}"));
        }
        let operation = wide("open");
        let file = wide(url);
        // ShellExecuteW returns a pseudo-HINSTANCE; anything above 32 signals success.
        let result = unsafe {
            ShellExecuteW(
                null_mut(),
                operation.as_ptr(),
                file.as_ptr(),
                null(),
                null(),
                SW_SHOWNORMAL,
            )
        };
        if (result as isize) > 32 {
            Ok(())
        } else {
            Err(format!("ShellExecuteW failed: {result:?}"))
        }
    }

    /// (friendly name, if_type, ipv4 string) for every adapter holding an IPv4 unicast address.
    pub fn lan_candidates() -> Vec<(String, u32, String)> {
        use std::net::Ipv4Addr;
        use windows_sys::Win32::Foundation::ERROR_BUFFER_OVERFLOW;
        use windows_sys::Win32::NetworkManagement::IpHelper::{
            GetAdaptersAddresses, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER,
            GAA_FLAG_SKIP_MULTICAST, IP_ADAPTER_ADDRESSES_LH, IP_ADAPTER_UNICAST_ADDRESS_LH,
        };
        use windows_sys::Win32::Networking::WinSock::{AF_INET, SOCKADDR, SOCKADDR_IN};

        fn pwstr_to_string(pointer: *const u16) -> String {
            if pointer.is_null() {
                return String::new();
            }
            let mut length = 0usize;
            unsafe {
                while *pointer.add(length) != 0 {
                    length += 1;
                }
            }
            String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(pointer, length) })
        }

        let mut size: u32 = 16 * 1024;
        let mut buffer: Vec<u64>;
        loop {
            buffer = vec![0; size.div_ceil(8) as usize];
            let status = unsafe {
                GetAdaptersAddresses(
                    AF_INET as u32,
                    GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER,
                    null_mut(),
                    buffer.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH,
                    &mut size,
                )
            };
            if status == ERROR_BUFFER_OVERFLOW {
                continue;
            }
            if status != 0 {
                return Vec::new();
            }
            break;
        }

        let mut candidates = Vec::new();
        let mut adapter = buffer.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
        while !adapter.is_null() {
            let name = pwstr_to_string(unsafe { (*adapter).FriendlyName });
            let if_type = unsafe { (*adapter).IfType };
            let mut unicast =
                unsafe { (*adapter).FirstUnicastAddress } as *const IP_ADAPTER_UNICAST_ADDRESS_LH;
            while !unicast.is_null() {
                let sockaddr = unsafe { (*unicast).Address.lpSockaddr } as *const SOCKADDR;
                if !sockaddr.is_null() && unsafe { (*sockaddr).sa_family } == AF_INET {
                    let sin = unsafe { &*(sockaddr as *const SOCKADDR_IN) };
                    // S_addr 保存的是网络字节序，先转回主机序数值，Ipv4Addr 才能按 a.b.c.d 解读
                    let address = u32::from_be(unsafe { sin.sin_addr.S_un.S_addr });
                    candidates.push((name.clone(), if_type, Ipv4Addr::from(address).to_string()));
                }
                unicast = unsafe { (*unicast).Next };
            }
            adapter = unsafe { (*adapter).Next };
        }
        candidates
    }

    unsafe extern "system" fn clipboard_window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if message == WM_CLIPBOARDUPDATE {
            if let Ok(listener) = CLIPBOARD_LISTENER.lock() {
                if let Some(handle) = listener.as_ref() {
                    let _ = handle.sender.send(());
                }
            }
            return 0;
        }
        unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
    }

    pub fn left_mouse_button_down() -> bool {
        unsafe { (GetAsyncKeyState(VK_LBUTTON as i32) & i16::MIN) != 0 }
    }

    unsafe extern "system" fn mouse_click_proc(
        code: i32,
        message: WPARAM,
        data: LPARAM,
    ) -> LRESULT {
        if code >= 0 && matches!(message as u32, WM_LBUTTONDOWN | WM_RBUTTONDOWN) {
            let event = unsafe { &*(data as *const MSLLHOOKSTRUCT) };
            if let Ok(sender) = MOUSE_CLICKS.lock() {
                if let Some(sender) = sender.as_ref() {
                    let at = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as i64;
                    let _ = sender.send((event.pt.x, event.pt.y, at));
                }
            }
        }
        unsafe { CallNextHookEx(null_mut(), code, message, data) }
    }

    pub fn start_mouse_click_notifications() -> Result<mpsc::Receiver<(i32, i32, i64)>, String> {
        let (sender, receiver) = mpsc::channel();
        *MOUSE_CLICKS.lock().map_err(|error| error.to_string())? = Some(sender);
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        thread::spawn(move || unsafe {
            let hook = SetWindowsHookExW(
                WH_MOUSE_LL,
                Some(mouse_click_proc),
                GetModuleHandleW(null()),
                0,
            );
            if hook.is_null() {
                let _ = ready_sender.send(Err(std::io::Error::last_os_error().to_string()));
                return;
            }
            let _ = ready_sender.send(Ok(()));
            let mut message = MSG::default();
            while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            UnhookWindowsHookEx(hook);
        });
        ready_receiver
            .recv_timeout(Duration::from_secs(2))
            .map_err(|error| error.to_string())??;
        Ok(receiver)
    }

    pub fn start_clipboard_notifications() -> Result<mpsc::Receiver<()>, String> {
        let (event_sender, event_receiver) = mpsc::channel();
        {
            let mut listener = CLIPBOARD_LISTENER
                .lock()
                .map_err(|_| "clipboard listener lock poisoned".to_string())?;
            if listener.is_some() {
                return Err("clipboard listener already started".to_string());
            }
            *listener = Some(ListenerHandle {
                sender: event_sender,
                window: 0,
            });
        }

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
            if let Ok(mut listener) = CLIPBOARD_LISTENER.lock() {
                if let Some(handle) = listener.as_mut() {
                    handle.window = hwnd as isize;
                }
            }

            let _ = ready_sender.send(Ok(()));
            let mut message: MSG = std::mem::zeroed();
            while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            RemoveClipboardFormatListener(hwnd);
            DestroyWindow(hwnd);
            // Only clear the slot if it still names our window; a restart may already
            // have replaced us.
            if let Ok(mut listener) = CLIPBOARD_LISTENER.lock() {
                if listener
                    .as_ref()
                    .is_some_and(|handle| handle.window == hwnd as isize)
                {
                    *listener = None;
                }
            }
        });

        let result = match ready_receiver.recv_timeout(Duration::from_secs(2)) {
            Ok(Ok(())) => Ok(event_receiver),
            Ok(Err(error)) => Err(error),
            Err(error) => Err(format!("clipboard listener startup timed out: {error}")),
        };
        if result.is_err() {
            // Startup failed or timed out: drop the pre-registered slot so a retry can
            // rebuild from a clean state.
            if let Ok(mut listener) = CLIPBOARD_LISTENER.lock() {
                *listener = None;
            }
        }
        result
    }

    /// Tear the current listener down: the event channel closes (stopping the monitor) and
    /// the message window is asked to unregister itself. A subsequent
    /// `start_clipboard_notifications` builds a fresh pair.
    pub fn stop_clipboard_notifications() {
        let handle = CLIPBOARD_LISTENER
            .lock()
            .ok()
            .and_then(|mut listener| listener.take());
        if let Some(handle) = handle {
            if handle.window != 0 {
                unsafe { PostMessageW(handle.window as HWND, WM_QUIT, 0, 0) };
            }
        }
    }

    const WATCHDOG_FORMAT: &str = "WitchClipboardWatchdog";

    /// Append a private marker format to the clipboard without emptying it. The user's
    /// current content stays untouched while the resulting update event proves the whole
    /// capture path alive.
    pub fn write_clipboard_probe() -> bool {
        let wide_name = wide(WATCHDOG_FORMAT);
        let format = unsafe { RegisterClipboardFormatW(wide_name.as_ptr()) };
        if format == 0 {
            return false;
        }
        let memory = unsafe { GlobalAlloc(GMEM_MOVEABLE, 4) };
        if memory.is_null() {
            return false;
        }
        let locked = unsafe { GlobalLock(memory) };
        if locked.is_null() {
            unsafe { GlobalFree(memory) };
            return false;
        }
        unsafe {
            std::ptr::write_volatile(locked as *mut u32, 1);
            GlobalUnlock(memory);
        }
        let Some(_guard) = ClipboardGuard::open() else {
            unsafe { GlobalFree(memory) };
            return false;
        };
        // No EmptyClipboard here on purpose: appending must not disturb current content.
        if unsafe { SetClipboardData(format, memory as *mut c_void) }.is_null() {
            unsafe { GlobalFree(memory) };
            return false;
        }
        // Ask system-level listeners (clipboard history, cloud sync) to skip this update:
        // the probe carries no user content and must not reshuffle Win+V history every
        // five minutes. The user's content was recorded when it was first copied.
        let exclude_name = wide("ExcludeClipboardContentFromMonitorProcessing");
        let exclude_format = unsafe { RegisterClipboardFormatW(exclude_name.as_ptr()) };
        if exclude_format != 0 {
            let flag = unsafe { GlobalAlloc(GMEM_MOVEABLE, 4) };
            if !flag.is_null() {
                let locked = unsafe { GlobalLock(flag) };
                if !locked.is_null() {
                    unsafe {
                        std::ptr::write_volatile(locked as *mut u32, 0);
                        GlobalUnlock(flag);
                    }
                    if unsafe { SetClipboardData(exclude_format, flag as *mut c_void) }.is_null() {
                        unsafe { GlobalFree(flag) };
                    }
                } else {
                    unsafe { GlobalFree(flag) };
                }
            }
        }
        true
    }

    pub fn clipboard_sequence() -> u32 {
        unsafe { GetClipboardSequenceNumber() }
    }

    pub fn foreground_window() -> Option<isize> {
        let hwnd = unsafe { GetForegroundWindow() };
        (!hwnd.is_null()).then_some(hwnd as isize)
    }

    pub fn is_paste_target(hwnd: isize) -> bool {
        let window = hwnd as HWND;
        if hwnd == 0
            || unsafe { IsWindow(window) == 0 || IsWindowVisible(window) == 0 }
            || window_process_id(hwnd).is_none_or(|pid| pid == std::process::id())
        {
            return false;
        }
        let mut class = [0u16; 256];
        let length = unsafe { GetClassNameW(window, class.as_mut_ptr(), class.len() as i32) };
        length > 0
            && !matches!(
                String::from_utf16_lossy(&class[..length as usize]).as_str(),
                "Shell_TrayWnd"
                    | "Shell_SecondaryTrayWnd"
                    | "NotifyIconOverflowWindow"
                    | "Progman"
                    | "WorkerW"
            )
    }

    pub fn show_panel(window: &tauri::WebviewWindow, preserve_focus: bool) -> Result<(), String> {
        if !preserve_focus {
            window.show().map_err(|error| error.to_string())?;
            return window.set_focus().map_err(|error| error.to_string());
        }
        let hwnd = window.hwnd().map_err(|error| error.to_string())?.0 as HWND;
        // Showing a window with SW_SHOW activates it even without a subsequent focus call.
        if unsafe {
            SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_SHOWWINDOW | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(())
    }

    pub fn mouse_position() -> Option<(i32, i32)> {
        let mut point = POINT::default();
        (unsafe { GetCursorPos(&mut point) != 0 }).then_some((point.x, point.y))
    }

    pub fn hide_panel(window: &tauri::WebviewWindow) {
        let _ = window.hide();
        // Native inactive-show does not update Tao's cached visibility flags, so its
        // hide request can be a no-op. Keep the real HWND hidden in both show modes.
        if let Ok(hwnd) = window.hwnd() {
            unsafe { ShowWindow(hwnd.0 as HWND, SW_HIDE) };
        }
    }

    pub fn point_inside_window(window: &tauri::WebviewWindow, point: (i32, i32)) -> bool {
        let Ok(hwnd) = window.hwnd() else { return true };
        let mut rect = RECT::default();
        if unsafe { GetWindowRect(hwnd.0 as HWND, &mut rect) == 0 } {
            return true;
        }
        point.0 >= rect.left && point.0 < rect.right && point.1 >= rect.top && point.1 < rect.bottom
    }

    pub fn window_process_id(hwnd: isize) -> Option<u32> {
        if hwnd == 0 {
            return None;
        }
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(hwnd as HWND, &mut pid) };
        (pid != 0).then_some(pid)
    }

    fn exe_name_from_pid(pid: u32) -> Option<String> {
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

    pub fn foreground_exe() -> Option<String> {
        let hwnd = foreground_window()?;
        let pid = window_process_id(hwnd)?;
        exe_name_from_pid(pid)
    }

    /// Process that last wrote the clipboard. Attribution must follow the writer, not the
    /// foreground window: copies from background processes and fast window switches would
    /// otherwise be attributed to the wrong source.
    pub fn clipboard_owner_exe() -> Option<String> {
        let owner = unsafe { GetClipboardOwner() };
        if owner.is_null() {
            return None;
        }
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(owner, &mut pid) };
        if pid == 0 {
            return None;
        }
        usable_owner_name(exe_name_from_pid(pid))
    }

    fn usable_owner_name(name: Option<String>) -> Option<String> {
        // UWP apps surface RuntimeBroker.exe as the clipboard owner, which says nothing
        // about the real source; reject it so callers fall back to the foreground window.
        match name {
            Some(name) if !name.eq_ignore_ascii_case("RuntimeBroker.exe") => Some(name),
            _ => None,
        }
    }

    pub fn clipboard_has_files() -> bool {
        unsafe { IsClipboardFormatAvailable(CF_HDROP) != 0 }
    }

    pub fn clipboard_has_text() -> bool {
        unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT) != 0 }
    }

    pub fn clipboard_has_image() -> bool {
        unsafe {
            IsClipboardFormatAvailable(CF_DIBV5) != 0 || IsClipboardFormatAvailable(CF_DIB) != 0
        }
    }

    pub fn clipboard_marked_sensitive() -> bool {
        let presence = SENSITIVE_PRESENCE_FORMATS
            .iter()
            .map(|name| format_available_by_name(name))
            .collect::<Vec<_>>();
        let history_flag = registered_format_dword(HISTORY_FLAG_FORMAT);
        sensitive_marker_decision(&presence, history_flag)
    }

    fn format_available_by_name(name: &str) -> bool {
        let wide_name = wide(name);
        let format = unsafe { RegisterClipboardFormatW(wide_name.as_ptr()) };
        format != 0 && unsafe { IsClipboardFormatAvailable(format) } != 0
    }

    fn registered_format_dword(name: &str) -> Option<u32> {
        let wide_name = wide(name);
        let format = unsafe { RegisterClipboardFormatW(wide_name.as_ptr()) };
        if format == 0 {
            return None;
        }
        let _guard = ClipboardGuard::open()?;
        let handle = unsafe { GetClipboardData(format) };
        if handle.is_null() {
            return None;
        }
        let locked = unsafe { GlobalLock(handle) };
        if locked.is_null() {
            return None;
        }
        let value = unsafe { std::ptr::read_volatile(locked as *const u32) };
        unsafe { GlobalUnlock(handle) };
        Some(value)
    }

    /// Pure decision core of `clipboard_marked_sensitive`, kept separate so the value
    /// semantics of the history flag stay pinned by unit tests.
    fn sensitive_marker_decision(presence: &[bool], history_flag: Option<u32>) -> bool {
        presence.iter().any(|flag| *flag) || history_flag == Some(0)
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
        if !is_paste_target(target) {
            return Err("no-target");
        }
        let target = target as HWND;
        // A minimized target would accept the keystrokes invisibly; restore it first.
        if unsafe { IsIconic(target) } != 0 {
            unsafe { ShowWindow(target, SW_RESTORE) };
        }
        // Synthesized input is silently blocked by UIPI when crossing from a normal process
        // into an elevated one; an explicit failure beats a paste that looks dead.
        if window_pid_is_elevated(target) && !current_process_is_elevated() {
            return Err("target-elevated");
        }
        // Let the panel finish hiding so it does not fight the upcoming z-order change.
        thread::sleep(Duration::from_millis(50));
        if !activate_target(target) {
            thread::sleep(Duration::from_millis(80));
            if !activate_target(target) {
                return Err("focus-failed");
            }
        }
        // The target is foreground now; give its message loop a beat to restore the focused
        // control before the keystroke lands.
        thread::sleep(Duration::from_millis(30));
        unsafe {
            for key in [VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN, VK_CONTROL] {
                if (GetAsyncKeyState(key as i32) & i16::MIN) != 0 {
                    if !send_key_event(key, KEYEVENTF_KEYUP) {
                        return Err("send-failed");
                    }
                }
            }
        }
        cancel_target_menu(target)?;
        if unsafe { GetForegroundWindow() != target } {
            return Err("focus-failed");
        }
        let inputs = [
            keyboard_input(VK_CONTROL, 0),
            keyboard_input(VK_V as u16, 0),
            keyboard_input(VK_V as u16, KEYEVENTF_KEYUP),
            keyboard_input(VK_CONTROL, KEYEVENTF_KEYUP),
        ];
        if unsafe {
            SendInput(
                inputs.len() as u32,
                inputs.as_ptr(),
                std::mem::size_of::<INPUT>() as i32,
            )
        } != inputs.len() as u32
        {
            // A partial insertion must not leave Control or V held down.
            let _ = send_key_event(VK_V as u16, KEYEVENTF_KEYUP);
            let _ = send_key_event(VK_CONTROL, KEYEVENTF_KEYUP);
            return Err("send-failed");
        }
        Ok(())
    }

    fn cancel_target_menu(target: HWND) -> Result<(), &'static str> {
        let mut info = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        let target_thread = unsafe { GetWindowThreadProcessId(target, null_mut()) };
        // Cancel menu/capture modes without dismissing a dialog or changing the caret.
        if unsafe { GetGUIThreadInfo(target_thread, &mut info) } != 0 {
            for window in [target, info.hwndFocus, info.hwndMenuOwner] {
                if !window.is_null() {
                    unsafe {
                        SendMessageTimeoutW(
                            window,
                            WM_CANCELMODE,
                            0,
                            0,
                            SMTO_ABORTIFHUNG,
                            100,
                            null_mut(),
                        )
                    };
                }
            }
        }
        cancel_custom_menu(target)
    }

    fn cancel_custom_menu(target: HWND) -> Result<(), &'static str> {
        use windows::Win32::{
            System::Com::{
                CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
                COINIT_MULTITHREADED,
            },
            UI::Accessibility::{CUIAutomation8, IUIAutomation2},
        };
        // Modern access-key menus are not represented by GUITHREADINFO. Inspect only
        // control types in the target process; never send Escape to an ordinary dialog.
        if unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_err() {
            return Ok(());
        }
        struct Apartment;
        impl Drop for Apartment {
            fn drop(&mut self) {
                unsafe { CoUninitialize() };
            }
        }
        let _apartment = Apartment;
        let Ok(automation) = (unsafe {
            CoCreateInstance::<_, IUIAutomation2>(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)
        }) else {
            return Ok(());
        };
        if unsafe {
            automation
                .SetConnectionTimeout(200)
                .and_then(|_| automation.SetTransactionTimeout(200))
        }
        .is_err()
        {
            return Ok(());
        }
        let Some(pid) = window_process_id(target as isize) else {
            return Err("no-target");
        };
        for _ in 0..3 {
            if unsafe { GetForegroundWindow() != target } {
                return Err("focus-failed");
            }
            if !focused_menu(&automation, pid).unwrap_or(false) {
                return Ok(());
            }
            if !send_key_event(VK_ESCAPE, 0) || !send_key_event(VK_ESCAPE, KEYEVENTF_KEYUP) {
                let _ = send_key_event(VK_ESCAPE, KEYEVENTF_KEYUP);
                return Err("send-failed");
            }
            thread::sleep(Duration::from_millis(30));
        }
        if focused_menu(&automation, pid).unwrap_or(false) {
            Err("focus-failed")
        } else {
            Ok(())
        }
    }

    fn focused_menu(
        automation: &windows::Win32::UI::Accessibility::IUIAutomation2,
        pid: u32,
    ) -> windows::core::Result<bool> {
        use windows::Win32::UI::Accessibility::{
            UIA_MenuBarControlTypeId, UIA_MenuControlTypeId, UIA_MenuItemControlTypeId,
            UIA_WindowControlTypeId,
        };
        unsafe {
            let mut element = automation.GetFocusedElement()?;
            let walker = automation.ControlViewWalker()?;
            for _ in 0..6 {
                if element.CurrentProcessId()? != pid as i32 {
                    return Ok(false);
                }
                let kind = element.CurrentControlType()?;
                if kind == UIA_MenuBarControlTypeId
                    || kind == UIA_MenuControlTypeId
                    || kind == UIA_MenuItemControlTypeId
                {
                    return Ok(true);
                }
                if kind == UIA_WindowControlTypeId {
                    break;
                }
                element = walker.GetParentElement(&element)?;
            }
        }
        Ok(false)
    }

    /// Force `target` into the foreground and wait for the activation to actually complete;
    /// `SetForegroundWindow` alone is regularly refused when the caller does not own the
    /// foreground (a panel that never received focus, background paste paths).
    fn activate_target(target: HWND) -> bool {
        if unsafe { GetForegroundWindow() == target } {
            return true;
        }
        unsafe {
            // Neutralize the foreground lock timeout for the duration of the switch, then
            // restore the user's original value.
            let mut lock_timeout: usize = 0;
            SystemParametersInfoW(
                SPI_GETFOREGROUNDLOCKTIMEOUT,
                0,
                &mut lock_timeout as *mut _ as *mut c_void,
                0,
            );
            SystemParametersInfoW(SPI_SETFOREGROUNDLOCKTIMEOUT, 0, std::ptr::null_mut(), 0);

            let foreground_thread = GetWindowThreadProcessId(GetForegroundWindow(), null_mut());
            let current_thread = GetCurrentThreadId();
            let attached = foreground_thread != 0
                && foreground_thread != current_thread
                && AttachThreadInput(current_thread, foreground_thread, 1) != 0;

            BringWindowToTop(target);
            let requested = SetForegroundWindow(target) != 0;

            if attached {
                AttachThreadInput(current_thread, foreground_thread, 0);
            }
            SystemParametersInfoW(
                SPI_SETFOREGROUNDLOCKTIMEOUT,
                lock_timeout as u32,
                std::ptr::null_mut(),
                0,
            );
            if !requested && GetForegroundWindow() != target {
                return false;
            }
        }
        // Activation is asynchronous and heavy applications finish it late; poll instead of
        // trusting a fixed sleep.
        for _ in 0..25 {
            if unsafe { GetForegroundWindow() == target } {
                return true;
            }
            thread::sleep(Duration::from_millis(10));
        }
        unsafe { GetForegroundWindow() == target }
    }

    fn window_pid_is_elevated(window: HWND) -> bool {
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(window, &mut pid) };
        pid != 0 && pid_is_elevated(pid)
    }

    fn current_process_is_elevated() -> bool {
        pid_is_elevated(std::process::id())
    }

    fn pid_is_elevated(pid: u32) -> bool {
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if process.is_null() {
                return false;
            }
            let mut token = std::ptr::null_mut();
            let opened = OpenProcessToken(process, TOKEN_QUERY, &mut token);
            CloseHandle(process);
            if opened == 0 || token.is_null() {
                return false;
            }
            let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
            let mut returned = 0u32;
            let ok = GetTokenInformation(
                token,
                TokenElevation,
                &mut elevation as *mut TOKEN_ELEVATION as *mut c_void,
                std::mem::size_of::<TOKEN_ELEVATION>() as u32,
                &mut returned,
            );
            CloseHandle(token);
            ok != 0 && elevation.TokenIsElevated != 0
        }
    }

    /// Inject one keystroke via SendInput, preferring scan codes: applications that read
    /// hardware scan codes (some terminals, nested remote sessions) ignore virtual-key
    /// events. Keys without a base scan code fall back to the virtual-key path.
    fn keyboard_input(vk: u16, flags: u32) -> INPUT {
        let scan = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) } as u16;
        let mut input: INPUT = unsafe { std::mem::zeroed() };
        input.r#type = INPUT_KEYBOARD;
        input.Anonymous.ki = if scan != 0 {
            KEYBDINPUT {
                wVk: 0,
                wScan: scan,
                dwFlags: flags | KEYEVENTF_SCANCODE,
                time: 0,
                dwExtraInfo: 0,
            }
        } else {
            KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            }
        };
        input
    }

    fn send_key_event(vk: u16, flags: u32) -> bool {
        let input = keyboard_input(vk, flags);
        unsafe { SendInput(1, &input, std::mem::size_of::<INPUT>() as i32) == 1 }
    }

    struct ClipboardGuard;

    impl ClipboardGuard {
        fn open() -> Option<Self> {
            for attempt in 0..OPEN_CLIPBOARD_ATTEMPTS {
                if unsafe { OpenClipboard(null_mut()) } != 0 {
                    return Some(Self);
                }
                if attempt + 1 < OPEN_CLIPBOARD_ATTEMPTS {
                    thread::sleep(OPEN_CLIPBOARD_RETRY_DELAY);
                }
            }
            None
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
        fn missing_windows_and_shell_cannot_be_paste_targets() {
            use windows_sys::Win32::UI::WindowsAndMessaging::FindWindowW;
            assert!(!is_paste_target(0));
            assert!(!is_paste_target(isize::MAX));
            assert_eq!(restore_and_paste(0), Err("no-target"));
            for class in [
                "Shell_TrayWnd",
                "Shell_SecondaryTrayWnd",
                "NotifyIconOverflowWindow",
                "Progman",
                "WorkerW",
            ] {
                let name = wide(class);
                let window = unsafe { FindWindowW(name.as_ptr(), null()) };
                if !window.is_null() {
                    assert!(
                        !is_paste_target(window as isize),
                        "{class} must not receive paste"
                    );
                }
            }
        }

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

        #[test]
        fn sensitive_marker_decision_respects_history_flag_value_semantics() {
            // History flag of 0 opts out; non-zero is an explicit opt-in and must not skip.
            assert!(sensitive_marker_decision(&[false, false], Some(0)));
            assert!(!sensitive_marker_decision(&[false, false], Some(1)));
            assert!(!sensitive_marker_decision(&[false, false], None));
            // Presence-only markers always opt out, regardless of the history flag.
            assert!(sensitive_marker_decision(&[true, false], Some(1)));
            assert!(sensitive_marker_decision(&[false, true], None));
            assert!(!sensitive_marker_decision(
                &[false, false, false],
                Some(u32::MAX)
            ));
        }

        #[test]
        fn owner_name_rejects_uwp_broker_and_missing_values() {
            assert_eq!(
                usable_owner_name(Some("msedge.exe".to_string())).as_deref(),
                Some("msedge.exe")
            );
            assert_eq!(
                usable_owner_name(Some("RuntimeBroker.exe".to_string())),
                None
            );
            assert_eq!(
                usable_owner_name(Some("runtimebroker.exe".to_string())),
                None
            );
            assert_eq!(usable_owner_name(None), None);
        }

        #[test]
        fn open_url_rejects_schemes_that_could_launch_arbitrary_targets() {
            fn allowed(url: &str) -> bool {
                is_allowed_external_url(url)
            }
            assert!(!allowed("file:///C:/Windows/System32/calc.exe"));
            assert!(!allowed("\\\\server\\share\\tool.exe"));
            assert!(!allowed("ms-settings:display"));
            assert!(allowed("https://www.witchcat.cn"));
            assert!(allowed("HTTPS://www.witchcat.cn"));
            assert!(allowed("mailto:witchscottishfoldcat@gmail.com"));
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use std::sync::mpsc;

    pub fn start_clipboard_notifications() -> Result<mpsc::Receiver<()>, String> {
        Err("native clipboard notifications are only available on Windows".to_string())
    }
    pub fn stop_clipboard_notifications() {}
    pub fn write_clipboard_probe() -> bool {
        false
    }
    pub fn clipboard_sequence() -> u32 {
        0
    }
    pub fn foreground_window() -> Option<isize> {
        None
    }
    pub fn is_paste_target(_hwnd: isize) -> bool {
        false
    }
    pub fn start_mouse_click_notifications() -> Result<mpsc::Receiver<(i32, i32, i64)>, String> {
        Err("native mouse notifications are only available on Windows".to_string())
    }
    pub fn mouse_position() -> Option<(i32, i32)> {
        None
    }
    pub fn hide_panel(window: &tauri::WebviewWindow) {
        let _ = window.hide();
    }
    pub fn point_inside_window(_window: &tauri::WebviewWindow, _point: (i32, i32)) -> bool {
        true
    }
    pub fn show_panel(window: &tauri::WebviewWindow, preserve_focus: bool) -> Result<(), String> {
        window.show().map_err(|error| error.to_string())?;
        if !preserve_focus {
            window.set_focus().map_err(|error| error.to_string())?;
        }
        Ok(())
    }
    pub fn window_process_id(_hwnd: isize) -> Option<u32> {
        None
    }
    pub fn foreground_exe() -> Option<String> {
        None
    }
    pub fn clipboard_owner_exe() -> Option<String> {
        None
    }
    pub fn clipboard_has_files() -> bool {
        false
    }
    pub fn clipboard_has_text() -> bool {
        false
    }
    pub fn clipboard_has_image() -> bool {
        false
    }
    pub fn clipboard_marked_sensitive() -> bool {
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
    pub fn open_url(_url: &str) -> Result<(), String> {
        Err("opening external URLs is only available on Windows".to_string())
    }
    pub fn lan_candidates() -> Vec<(String, u32, String)> {
        Vec::new()
    }
    pub fn left_mouse_button_down() -> bool {
        false
    }
}

pub use imp::*;
