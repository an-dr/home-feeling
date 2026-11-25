//! The Windows backend. One thread owns the event hook, the hotkeys, and the
//! overlay windows, because Win32 ties each of them to the thread that made
//! it; the module talks to that thread through channels.
use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Mutex, OnceLock};

use windows_sys::core::BOOL;
use windows_sys::Win32::Foundation::{
    CloseHandle, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM,
};
use windows_sys::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS,
};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CombineRgn, CreateCompatibleDC, CreateDIBSection, CreateFontW, CreateRectRgn,
    CreateRoundRectRgn, CreateSolidBrush, DeleteDC, DeleteObject, DrawTextW, EndPaint,
    EnumDisplayMonitors, FillRect, GetDC, GetMonitorInfoW, InvalidateRect, MonitorFromPoint,
    MonitorFromWindow, ReleaseDC, SelectObject, SetBkMode, SetTextColor, SetWindowRgn,
    AC_SRC_ALPHA, AC_SRC_OVER, ANTIALIASED_QUALITY, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    BLENDFUNCTION, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH, DIB_RGB_COLORS,
    DT_CALCRECT, DT_CENTER, DT_END_ELLIPSIS, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, FW_BOLD,
    FW_NORMAL, FW_SEMIBOLD, HDC, HMONITOR, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    OUT_DEFAULT_PRECIS, PAINTSTRUCT, RGN_DIFF, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows_sys::Win32::System::SystemInformation::GetTickCount;
use windows_sys::Win32::System::Threading::{
    GetCurrentProcessId, GetCurrentThreadId, OpenProcess, QueryFullProcessImageNameW,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    keybd_event, GetAsyncKeyState, GetKeyState, RegisterHotKey, ReleaseCapture, SetCapture,
    SetFocus, UnregisterHotKey, KEYEVENTF_KEYUP, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT,
    MOD_WIN, VK_BACK, VK_CAPITAL, VK_CONTROL, VK_DELETE, VK_DOWN, VK_ESCAPE, VK_F1, VK_LEFT,
    VK_LWIN, VK_MENU, VK_RETURN, VK_RIGHT, VK_RWIN, VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::keys::{Hotkey, Key, Modifiers};
use crate::protocol::{
    Capabilities, Id, MenuItem, Monitor, Overlay, Point, Rect, Snapshot, WindowInfo,
};
use crate::{Backend, RawEvent};

/// Wakes the thread to read its command channel.
const WM_COMMANDS: u32 = WM_APP + 1;

enum Command {
    RegisterHotkey(u32, Hotkey, Sender<Result<(), String>>),
    UnregisterHotkey(u32),
    SetOverlay(u32, Overlay),
    ClearOverlay(u32),
    ShowMenu(u32, Vec<MenuItem>, Point, String),
}

/// Raw notifications from the hook and the message loop.
enum Note {
    Window(u32, isize),
    Hotkey(u32),
    Display,
    OverlayClicked(u32),
    Menu(u32, Option<u32>),
}

static NOTES: OnceLock<Mutex<Sender<Note>>> = OnceLock::new();

fn note(note: Note) {
    if let Some(sender) = NOTES.get() {
        let _ = sender.lock().unwrap().send(note);
    }
}

pub struct Windows {
    thread_id: u32,
    commands: Sender<Command>,
    notes: Receiver<Note>,
    /// Managed windows, by id.
    known: HashMap<Id, WindowInfo>,
    focused: Option<Id>,
    monitors: Vec<Monitor>,
    /// Windows moved on request, and when to report the frame they took.
    checks: Vec<(Id, std::time::Instant, Rect)>,
    /// The window the user is dragging or resizing, if any.
    dragging: Option<Id>,
}

/// How long a window gets to settle after a move before its frame is
/// reported, whether or not it changed.
const SETTLE: std::time::Duration = std::time::Duration::from_millis(150);

impl Windows {
    pub fn start() -> Self {
        // Window frames come back in real pixels, so monitors must too. The
        // engine turns DPI awareness on only when it opens its window, after
        // this runs; on a scaled display monitors read earlier are scaled.
        unsafe {
            windows_sys::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
                windows_sys::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            );
        }
        let (note_tx, notes) = channel();
        let _ = NOTES.set(Mutex::new(note_tx));
        let (commands, command_rx) = channel();
        let (id_tx, id_rx) = channel();
        std::thread::Builder::new()
            .name("desktop-events".into())
            .spawn(move || event_thread(command_rx, id_tx))
            .expect("could not start the desktop event thread");
        let thread_id = id_rx.recv().unwrap_or(0);
        let mut windows = Self {
            thread_id,
            commands,
            notes,
            known: HashMap::new(),
            focused: None,
            monitors: monitors(),
            checks: Vec::new(),
            dragging: None,
        };
        for hwnd in top_level_windows() {
            if let Some(info) = describe(hwnd) {
                windows.known.insert(info.id, info);
            }
        }
        windows.focused = Some(unsafe { GetForegroundWindow() } as Id)
            .filter(|id| windows.known.contains_key(id));
        windows
    }

    fn command(&self, command: Command) {
        if self.commands.send(command).is_ok() {
            unsafe { PostThreadMessageW(self.thread_id, WM_COMMANDS, 0, 0) };
        }
    }

    fn on_window(&mut self, event: u32, hwnd: isize, out: &mut Vec<RawEvent>) {
        let id = hwnd as Id;
        match event {
            EVENT_OBJECT_DESTROY | EVENT_OBJECT_HIDE | EVENT_OBJECT_CLOAKED => {
                if self.known.remove(&id).is_some() {
                    out.push(RawEvent::Closed(id));
                }
            }
            EVENT_SYSTEM_MINIMIZESTART => {
                if let Some(info) = self.known.get_mut(&id) {
                    info.minimized = true;
                    out.push(RawEvent::Minimized(id));
                }
            }
            EVENT_OBJECT_SHOW
            | EVENT_OBJECT_UNCLOAKED
            | EVENT_SYSTEM_MINIMIZEEND
            | EVENT_SYSTEM_FOREGROUND => {
                let fresh = match self.known.get(&id) {
                    None => true,
                    Some(info) => info.minimized,
                };
                if fresh {
                    if let Some(info) = describe(hwnd as HWND) {
                        self.known.insert(id, info.clone());
                        out.push(RawEvent::Opened(info));
                    }
                }
                if event == EVENT_SYSTEM_FOREGROUND && self.known.contains_key(&id) {
                    self.focused = Some(id);
                    out.push(RawEvent::Focused(id));
                }
            }
            EVENT_SYSTEM_MOVESIZESTART => {
                if self.known.contains_key(&id) {
                    self.dragging = Some(id);
                    out.push(RawEvent::DragStarted(id, cursor()));
                }
            }
            EVENT_OBJECT_LOCATIONCHANGE | EVENT_SYSTEM_MOVESIZEEND => {
                if let Some(info) = self.known.get_mut(&id) {
                    let maximized = unsafe { IsZoomed(hwnd as HWND) } != 0;
                    if maximized != info.maximized {
                        info.maximized = maximized;
                        out.push(RawEvent::Maximized(id, maximized));
                    }
                    if let Some(rect) = frame(hwnd as HWND) {
                        if rect != info.rect {
                            info.rect = rect;
                            info.monitor = unsafe {
                                MonitorFromWindow(hwnd as HWND, MONITOR_DEFAULTTONEAREST)
                            } as Id;
                            out.push(RawEvent::Moved(id, rect));
                        }
                        // The end of a drag or resize by the user, as opposed
                        // to a move this module or the app made.
                        if event == EVENT_SYSTEM_MOVESIZEEND {
                            self.dragging = None;
                            out.push(RawEvent::Dragged(id, rect, cursor()));
                        } else if self.dragging == Some(id) {
                            out.push(RawEvent::Dragging(id, rect, cursor()));
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

impl Backend for Windows {
    fn snapshot(&mut self) -> Snapshot {
        // Read again: scale or layout may have changed since start.
        self.monitors = monitors();
        Snapshot {
            capabilities: Capabilities {
                windows: true,
                hotkeys: true,
                overlays: true,
            },
            platform: "windows".into(),
            windows: self
                .known
                .values()
                .filter(|w| !w.minimized)
                .cloned()
                .collect(),
            monitors: self.monitors.clone(),
            focused: self.focused,
        }
    }

    fn poll(&mut self) -> Vec<RawEvent> {
        let mut out = Vec::new();
        while let Ok(note) = self.notes.try_recv() {
            match note {
                Note::Window(event, hwnd) => self.on_window(event, hwnd, &mut out),
                Note::Hotkey(key) => out.push(RawEvent::Hotkey(key)),
                Note::OverlayClicked(key) => out.push(RawEvent::OverlayClicked(key)),
                Note::Menu(key, item) => out.push(RawEvent::Menu(key, item)),
                Note::Display => {
                    self.monitors = monitors();
                    out.push(RawEvent::MonitorsChanged(self.monitors.clone()));
                }
            }
        }
        // A window that refuses a move keeps its frame, so no change event
        // comes; report the frame anyway, so the requester learns the outcome.
        let now = std::time::Instant::now();
        let (due, waiting): (Vec<_>, Vec<_>) =
            self.checks.drain(..).partition(|(_, at, _)| now >= *at);
        self.checks = waiting;
        for (id, _, requested) in due {
            if let (Some(info), Some(rect)) = (self.known.get_mut(&id), frame(id as HWND)) {
                info.rect = rect;
                out.push(RawEvent::Settled(id, rect, requested));
            }
        }
        out
    }

    fn move_window(&mut self, id: Id, rect: Rect, raise: bool) -> Result<(), String> {
        let hwnd = id as HWND;
        unsafe {
            if IsWindow(hwnd) == 0 {
                return Err(format!("no window {id}"));
            }
            if IsZoomed(hwnd) != 0 {
                ShowWindow(hwnd, SW_RESTORE);
            }
            // A minimized window reports the size of its icon, and a move of it
            // only sets where its icon is: bring it back first, then measure.
            if IsIconic(hwnd) != 0 {
                ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            }
            // The visible frame is inset from the window rectangle by
            // invisible resize borders; grow the target by the same amount.
            let mut outer = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            GetWindowRect(hwnd, &mut outer);
            let visible = frame(hwnd).unwrap_or(Rect {
                x: outer.left,
                y: outer.top,
                w: outer.right - outer.left,
                h: outer.bottom - outer.top,
            });
            let left = visible.x - outer.left;
            let top = visible.y - outer.top;
            let right = outer.right - (visible.x + visible.w);
            let bottom = outer.bottom - (visible.y + visible.h);
            let ok = SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                rect.x - left,
                rect.y - top,
                rect.w + left + right,
                rect.h + top + bottom,
                SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
            );
            if ok == 0 {
                return Err(format!(
                    "could not move window {id}; it may run as administrator"
                ));
            }
            if raise {
                // Windows ignores HWND_TOP for another process's window while
                // that process is not in the foreground; passing through
                // topmost lifts it above every ordinary window all the same.
                let keep = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
                SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, keep);
                SetWindowPos(hwnd, HWND_NOTOPMOST, 0, 0, 0, 0, keep);
            }
        }
        self.checks.retain(|(other, _, _)| *other != id);
        self.checks
            .push((id, std::time::Instant::now() + SETTLE, rect));
        Ok(())
    }

    fn minimize(&mut self, id: Id) -> Result<(), String> {
        unsafe { ShowWindow(id as HWND, SW_SHOWMINNOACTIVE) };
        Ok(())
    }

    fn restore(&mut self, id: Id) -> Result<(), String> {
        unsafe { ShowWindow(id as HWND, SW_SHOWNOACTIVATE) };
        Ok(())
    }

    fn focus(&mut self, id: Id) -> Result<(), String> {
        let hwnd = id as HWND;
        unsafe {
            if IsIconic(hwnd) != 0 {
                ShowWindow(hwnd, SW_RESTORE);
            }
            if SetForegroundWindow(hwnd) == 0 {
                // Windows lets a process take the foreground right after a
                // key press; a tap of Alt counts as one.
                keybd_event(VK_MENU as u8, 0, 0, 0);
                keybd_event(VK_MENU as u8, 0, KEYEVENTF_KEYUP, 0);
                if SetForegroundWindow(hwnd) == 0 {
                    return Err(format!("could not focus window {id}"));
                }
            }
        }
        Ok(())
    }

    fn register_hotkey(&mut self, key: u32, hotkey: Hotkey) -> Result<(), String> {
        let (tx, rx) = channel();
        self.command(Command::RegisterHotkey(key, hotkey, tx));
        rx.recv()
            .unwrap_or_else(|_| Err("the desktop event thread stopped".into()))
    }

    fn unregister_hotkey(&mut self, key: u32) {
        self.command(Command::UnregisterHotkey(key));
    }

    fn set_overlay(&mut self, key: u32, overlay: &Overlay) {
        self.command(Command::SetOverlay(key, overlay.clone()));
    }

    fn clear_overlay(&mut self, key: u32) {
        self.command(Command::ClearOverlay(key));
    }

    fn show_menu(&mut self, key: u32, items: &[MenuItem], at: Point, accent: &str) {
        self.command(Command::ShowMenu(
            key,
            items.to_vec(),
            at,
            accent.to_string(),
        ));
    }
}

/// The cursor, in real pixels.
fn cursor() -> Point {
    let mut p = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
    unsafe { GetCursorPos(&mut p) };
    Point { x: p.x, y: p.y }
}

/// The visible frame, without invisible borders.
fn frame(hwnd: HWND) -> Option<Rect> {
    let mut r = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    let hr = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS as _,
            &mut r as *mut RECT as *mut c_void,
            std::mem::size_of::<RECT>() as u32,
        )
    };
    if hr != 0 {
        unsafe { GetWindowRect(hwnd, &mut r) };
    }
    Some(Rect {
        x: r.left,
        y: r.top,
        w: r.right - r.left,
        h: r.bottom - r.top,
    })
}

fn text(hwnd: HWND, class: bool) -> String {
    let mut buffer = [0u16; 256];
    let n = unsafe {
        if class {
            GetClassNameW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32)
        } else {
            GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32)
        }
    };
    String::from_utf16_lossy(&buffer[..n.max(0) as usize])
}

/// The window as a Thing sees it, if it is one an ordinary user would
/// arrange: visible, top-level, captioned, not a tool window, not cloaked,
/// not part of the shell, and not one of ours.
fn describe(hwnd: HWND) -> Option<WindowInfo> {
    unsafe {
        if hwnd.is_null() || IsWindowVisible(hwnd) == 0 {
            return None;
        }
        if GetAncestor(hwnd, GA_ROOT) != hwnd || !GetWindow(hwnd, GW_OWNER).is_null() {
            return None;
        }
        let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
        let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        if style & WS_CAPTION != WS_CAPTION || ex & (WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE) != 0 {
            return None;
        }
        let mut cloaked: u32 = 0;
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED as _,
            &mut cloaked as *mut u32 as *mut c_void,
            4,
        );
        if cloaked != 0 {
            return None;
        }
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == GetCurrentProcessId() {
            return None;
        }
        let title = text(hwnd, false);
        let class = text(hwnd, true);
        const SHELL: [&str; 5] = [
            "Progman",
            "WorkerW",
            "Shell_TrayWnd",
            "Shell_SecondaryTrayWnd",
            "Windows.UI.Core.CoreWindow",
        ];
        if title.is_empty() || SHELL.contains(&class.as_str()) {
            return None;
        }
        Some(WindowInfo {
            id: hwnd as Id,
            title,
            class,
            rect: frame(hwnd)?,
            monitor: MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) as Id,
            minimized: IsIconic(hwnd) != 0,
            maximized: IsZoomed(hwnd) != 0,
        })
    }
}

fn top_level_windows() -> Vec<HWND> {
    unsafe extern "system" fn collect(hwnd: HWND, list: LPARAM) -> BOOL {
        (*(list as *mut Vec<isize>)).push(hwnd as isize);
        1
    }
    let mut list: Vec<isize> = Vec::new();
    unsafe { EnumWindows(Some(collect), &mut list as *mut Vec<isize> as LPARAM) };
    list.into_iter().map(|h| h as HWND).collect()
}

fn monitors() -> Vec<Monitor> {
    unsafe extern "system" fn collect(
        monitor: HMONITOR,
        _: HDC,
        _: *mut RECT,
        list: LPARAM,
    ) -> BOOL {
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(monitor, &mut info) != 0 {
            let rect = |r: RECT| Rect {
                x: r.left,
                y: r.top,
                w: r.right - r.left,
                h: r.bottom - r.top,
            };
            (*(list as *mut Vec<Monitor>)).push(Monitor {
                id: monitor as Id,
                rect: rect(info.rcMonitor),
                work: rect(info.rcWork),
            });
        }
        1
    }
    let mut list: Vec<Monitor> = Vec::new();
    unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(collect),
            &mut list as *mut Vec<Monitor> as LPARAM,
        )
    };
    list
}

unsafe extern "system" fn on_event(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    object: i32,
    child: i32,
    _thread: u32,
    _time: u32,
) {
    if object == OBJID_WINDOW && child == CHILDID_SELF as i32 && !hwnd.is_null() {
        note(Note::Window(event, hwnd as isize));
        // Another window came forward, perhaps over a badge's window.
        restack_badges();
    }
}

/// Where a badge goes to sit just above `target`: the window to insert it
/// after, or `None` when it already is there.
unsafe fn place_above(target: HWND, badge: HWND) -> Option<HWND> {
    if IsWindow(target) == 0 {
        return None;
    }
    let topmost = |window: HWND| GetWindowLongPtrW(window, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST != 0;
    if topmost(target) {
        return Some(HWND_TOPMOST);
    }
    let above = GetWindow(target, GW_HWNDPREV);
    if above == badge {
        None
    } else if above.is_null() || topmost(above) {
        // Nothing ordinary is in front of the window; inserting after a
        // topmost one would make the badge topmost.
        Some(HWND_NOTOPMOST)
    } else {
        Some(above)
    }
}

/// Puts every badge back just above its window.
fn restack_badges() {
    let badges: Vec<(isize, isize)> = BADGES.with(|b| {
        b.borrow()
            .iter()
            .map(|(badge, target)| (*badge, *target))
            .collect()
    });
    for (badge, target) in badges {
        unsafe {
            if let Some(after) = place_above(target as HWND, badge as HWND) {
                SetWindowPos(
                    badge as HWND,
                    after,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
        }
    }
}

thread_local! {
    /// Each overlay's border window and its fill window, by key.
    static OVERLAYS: std::cell::RefCell<HashMap<u32, [isize; 2]>> = Default::default();
    static COLORS: std::cell::RefCell<HashMap<isize, u32>> = Default::default();
    /// Badge windows already switched to per-pixel alpha.
    static SMOOTH: std::cell::RefCell<std::collections::HashSet<isize>> = Default::default();
    /// The Caps Lock hotkeys by key, and the hook that takes the key over.
    static CAPS: std::cell::RefCell<Vec<(u32, Modifiers)>> = Default::default();
    static CAPS_HOOK: std::cell::Cell<isize> = const { std::cell::Cell::new(0) };
    static CAPS_DOWN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The window each badge sits above, by badge window.
    static BADGES: std::cell::RefCell<HashMap<isize, isize>> = Default::default();
}

/// One entry of a popup menu, as the owner draws it.
#[derive(Clone)]
struct MenuRow {
    title: String,
    /// The program the window belongs to.
    detail: String,
    checked: bool,
    icon: isize,
    letter: char,
}

/// Windows' scale for the display, as a factor of 96 dpi.
fn ui_scale() -> f32 {
    unsafe { windows_sys::Win32::UI::HiDpi::GetDpiForSystem() as f32 / 96.0 }
}

fn menu_font(pixels: i32, bold: bool) -> *mut c_void {
    unsafe {
        CreateFontW(
            -pixels,
            0,
            0,
            0,
            if bold { FW_SEMIBOLD } else { FW_NORMAL } as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            ANTIALIASED_QUALITY as u32,
            DEFAULT_PITCH as u32,
            wide("Segoe UI").as_ptr(),
        )
    }
}

fn colorref(hex: &str) -> u32 {
    let hex = hex.trim_start_matches('#');
    match u32::from_str_radix(hex, 16) {
        Ok(rgb) if hex.len() == 6 => ((rgb & 0xff) << 16) | (rgb & 0xff00) | (rgb >> 16),
        _ => 0x00d4_bc00,
    }
}

/// How wide a row's title is, so the menu fits the longest one.
unsafe fn title_width(title: &str, bold: bool, scale: f32) -> i32 {
    let screen = GetDC(std::ptr::null_mut());
    let font = menu_font((14.0 * scale).round() as i32, bold);
    let before = SelectObject(screen, font as _);
    let text = wide(title);
    let mut area = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    DrawTextW(
        screen,
        text.as_ptr(),
        -1,
        &mut area,
        DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX,
    );
    SelectObject(screen, before);
    DeleteObject(font as _);
    ReleaseDC(std::ptr::null_mut(), screen);
    area.right - area.left
}

/// The icon a window shows, or 0 when it will not say.
unsafe fn window_icon(window: HWND) -> isize {
    for which in [ICON_SMALL2, ICON_SMALL, ICON_BIG] {
        let mut icon: usize = 0;
        let sent = SendMessageTimeoutW(
            window,
            WM_GETICON,
            which as usize,
            0,
            SMTO_ABORTIFHUNG,
            80,
            &mut icon,
        );
        if sent != 0 && icon != 0 {
            return icon as isize;
        }
    }
    let class = GetClassLongPtrW(window, GCLP_HICONSM);
    if class != 0 {
        return class as isize;
    }
    GetClassLongPtrW(window, GCLP_HICON) as isize
}

/// The program a window belongs to, as its file's name without `.exe`.
unsafe fn program_name(window: HWND) -> String {
    let mut process = 0u32;
    GetWindowThreadProcessId(window, &mut process);
    let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process);
    if handle.is_null() {
        return String::new();
    }
    let mut path = [0u16; 520];
    let mut length = path.len() as u32;
    let read = QueryFullProcessImageNameW(handle, 0, path.as_mut_ptr(), &mut length);
    CloseHandle(handle);
    if read == 0 {
        return String::new();
    }
    let path = String::from_utf16_lossy(&path[..length as usize]);
    let file = path.rsplit(['\\', '/']).next().unwrap_or("");
    let stem = file.strip_suffix(".exe").unwrap_or(file);
    let mut chars = stem.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

unsafe extern "system" fn overlay_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut paint: PAINTSTRUCT = std::mem::zeroed();
            let dc = BeginPaint(hwnd, &mut paint);
            let color = COLORS.with(|c| c.borrow().get(&(hwnd as isize)).copied().unwrap_or(0));
            let brush = CreateSolidBrush(color);
            FillRect(dc, &paint.rcPaint, brush);
            DeleteObject(brush as _);
            EndPaint(hwnd, &paint);
            0
        }
        // A badge takes clicks without taking focus from the window beneath.
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        WM_SETCURSOR => {
            SetCursor(LoadCursorW(std::ptr::null_mut(), IDC_HAND));
            1
        }
        WM_LBUTTONUP => {
            let key = OVERLAYS.with(|o| {
                o.borrow()
                    .iter()
                    .find(|(_, windows)| windows[0] == hwnd as isize)
                    .map(|(key, _)| *key)
            });
            if let Some(key) = key {
                note(Note::OverlayClicked(key));
            }
            0
        }
        WM_DISPLAYCHANGE => {
            note(Note::Display);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn create_overlay_window(class: &[u16]) -> isize {
    unsafe {
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            class.as_ptr(),
            std::ptr::null(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            GetModuleHandleW(std::ptr::null()),
            std::ptr::null(),
        );
        SetLayeredWindowAttributes(hwnd, 0, 255, LWA_ALPHA);
        hwnd as isize
    }
}

/// A rectangle region of `w` by `h` at (`x`, `y`) in window coordinates, its
/// corners rounded by `radius` pixels when that is above zero.
unsafe fn shape(
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    radius: i32,
) -> windows_sys::Win32::Graphics::Gdi::HRGN {
    if radius > 0 {
        // Round-rect regions include their right and bottom edge.
        CreateRoundRectRgn(x, y, x + w + 1, y + h + 1, 2 * radius, 2 * radius)
    } else {
        CreateRectRgn(x, y, x + w, y + h)
    }
}

fn set_overlay(class: &[u16], key: u32, overlay: &Overlay) {
    let [border, fill] = OVERLAYS.with(|o| {
        *o.borrow_mut()
            .entry(key)
            .or_insert_with(|| std::array::from_fn(|_| create_overlay_window(class)))
    });
    let (r, g, b) = overlay.rgb();
    let color = u32::from(r) | (u32::from(g) << 8) | (u32::from(b) << 16);
    let Rect { x, y, w, h } = overlay.rect;
    let t = overlay.width.max(1);
    let radius = overlay.radius.max(0);
    COLORS.with(|c| {
        let mut c = c.borrow_mut();
        c.insert(border, color);
        c.insert(fill, color);
    });
    if overlay.badge.is_some() {
        unsafe {
            // A circle that takes clicks: the window stops being click-through.
            let style = GetWindowLongPtrW(border as HWND, GWL_EXSTYLE);
            SetWindowLongPtrW(
                border as HWND,
                GWL_EXSTYLE,
                style & !(WS_EX_TRANSPARENT as isize),
            );
            SetWindowRgn(border as HWND, std::ptr::null_mut(), 1);
            ShowWindow(fill as HWND, SW_HIDE);
            let target = overlay.attach.map(|id| id as isize);
            BADGES.with(|b| match target {
                Some(target) => {
                    b.borrow_mut().insert(border, target);
                }
                None => {
                    b.borrow_mut().remove(&border);
                }
            });
            let after = match target {
                Some(target) => place_above(target as HWND, border as HWND),
                None => Some(HWND_TOPMOST),
            };
            SetWindowPos(
                border as HWND,
                after.unwrap_or(std::ptr::null_mut()),
                x,
                y,
                w,
                h,
                SWP_NOACTIVATE | SWP_SHOWWINDOW | if after.is_none() { SWP_NOZORDER } else { 0 },
            );
            // Per-pixel alpha cannot be mixed with the whole-window alpha the
            // window was made with: switch the style off and on once.
            if SMOOTH.with(|m| m.borrow_mut().insert(border)) {
                let style = GetWindowLongPtrW(border as HWND, GWL_EXSTYLE);
                SetWindowLongPtrW(
                    border as HWND,
                    GWL_EXSTYLE,
                    style & !(WS_EX_LAYERED as isize),
                );
                SetWindowLongPtrW(border as HWND, GWL_EXSTYLE, style | WS_EX_LAYERED as isize);
            }
            render_badge(
                border as HWND,
                w,
                h,
                (r, g, b),
                overlay.badge.as_deref().unwrap_or(""),
            );
        }
        return;
    }
    unsafe {
        // The border is one window over the rect grown by its width, shaped
        // as a ring: the outer shape minus the rect itself.
        let (bw, bh) = (w + 2 * t, h + 2 * t);
        let ring = shape(0, 0, bw, bh, if radius > 0 { radius + t } else { 0 });
        let hole = shape(t, t, w, h, radius);
        CombineRgn(ring, ring, hole, RGN_DIFF);
        DeleteObject(hole as _);
        // The window owns the region from here on.
        SetWindowRgn(border as HWND, ring, 1);
        SetWindowPos(
            border as HWND,
            HWND_TOPMOST,
            x - t,
            y - t,
            bw,
            bh,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
        InvalidateRect(border as HWND, std::ptr::null(), 1);

        // The fill is the rect itself, at the fill's opacity.
        let opacity = overlay.fill.unwrap_or(0.0).clamp(0.0, 1.0);
        if opacity <= 0.0 || w <= 0 || h <= 0 {
            ShowWindow(fill as HWND, SW_HIDE);
        } else {
            SetLayeredWindowAttributes(fill as HWND, 0, (opacity * 255.0).round() as u8, LWA_ALPHA);
            SetWindowRgn(fill as HWND, shape(0, 0, w, h, radius), 1);
            SetWindowPos(
                fill as HWND,
                HWND_TOPMOST,
                x,
                y,
                w,
                h,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            InvalidateRect(fill as HWND, std::ptr::null(), 1);
        }
    }
}

/// Draws a badge into its layered window: an anti-aliased disc of the colour
/// with the label in white, blended by per-pixel alpha so its edge is smooth.
unsafe fn render_badge(hwnd: HWND, w: i32, h: i32, (r, g, b): (u8, u8, u8), label: &str) {
    if w <= 0 || h <= 0 {
        return;
    }
    let screen = GetDC(std::ptr::null_mut());
    let dc = CreateCompatibleDC(screen);
    let mut header: BITMAPINFO = std::mem::zeroed();
    header.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    header.bmiHeader.biWidth = w;
    header.bmiHeader.biHeight = -h; // top-down
    header.bmiHeader.biPlanes = 1;
    header.bmiHeader.biBitCount = 32;
    header.bmiHeader.biCompression = BI_RGB;
    let count = (w * h) as usize;

    // The text first, white on black with greyscale smoothing, as a mask.
    let mut bits: *mut c_void = std::ptr::null_mut();
    let mask = CreateDIBSection(
        dc,
        &header,
        DIB_RGB_COLORS,
        &mut bits,
        std::ptr::null_mut(),
        0,
    );
    let before = SelectObject(dc, mask as _);
    let font = CreateFontW(
        -(h * 11 / 20),
        0,
        0,
        0,
        FW_BOLD as i32,
        0,
        0,
        0,
        DEFAULT_CHARSET as u32,
        OUT_DEFAULT_PRECIS as u32,
        CLIP_DEFAULT_PRECIS as u32,
        ANTIALIASED_QUALITY as u32,
        DEFAULT_PITCH as u32,
        wide("Segoe UI").as_ptr(),
    );
    let old_font = SelectObject(dc, font as _);
    SetBkMode(dc, TRANSPARENT as i32);
    SetTextColor(dc, 0x00ff_ffff);
    let text = wide(label);
    let mut area = RECT {
        left: 0,
        top: 0,
        right: w,
        bottom: h,
    };
    DrawTextW(
        dc,
        text.as_ptr(),
        -1,
        &mut area,
        DT_CENTER | DT_VCENTER | DT_SINGLELINE,
    );
    SelectObject(dc, old_font);
    DeleteObject(font as _);
    let ink: Vec<u32> = std::slice::from_raw_parts(bits as *const u32, count).to_vec();
    SelectObject(dc, before);
    DeleteObject(mask as _);

    // The picture: coverage of the disc by distance from its centre, the text
    // mixed in, colours premultiplied as the blend asks.
    let picture = CreateDIBSection(
        dc,
        &header,
        DIB_RGB_COLORS,
        &mut bits,
        std::ptr::null_mut(),
        0,
    );
    let before = SelectObject(dc, picture as _);
    let pixels = std::slice::from_raw_parts_mut(bits as *mut u32, count);
    let radius = w.min(h) as f32 / 2.0;
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let opacity = 0.94;
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            let coverage = (radius - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0.0, 1.0);
            let letter = f32::from((ink[(y * w + x) as usize] & 0xff) as u8) / 255.0;
            let alpha = coverage * opacity;
            let mix = |base: u8| (f32::from(base) * (1.0 - letter) + 255.0 * letter) * alpha;
            pixels[(y * w + x) as usize] = ((alpha * 255.0).round() as u32) << 24
                | (mix(r).round() as u32) << 16
                | (mix(g).round() as u32) << 8
                | mix(b).round() as u32;
        }
    }
    let size = SIZE { cx: w, cy: h };
    let from = POINT { x: 0, y: 0 };
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    UpdateLayeredWindow(
        hwnd,
        screen,
        std::ptr::null(),
        &size,
        dc,
        &from,
        0,
        &blend,
        ULW_ALPHA,
    );
    SelectObject(dc, before);
    DeleteObject(picture as _);
    DeleteDC(dc);
    ReleaseDC(std::ptr::null_mut(), screen);
}

/// A popup menu this module draws itself: rounded, shadowed, and themed to
/// the system's light or dark mode, instead of Windows' own menu.
struct MenuState {
    hwnd: isize,
    key: u32,
    /// The ids reported for each row.
    ids: Vec<u32>,
    rows: Vec<MenuRow>,
    hover: Option<usize>,
    accent: [u8; 3],
    dark: bool,
    scale: f32,
    /// The rounded body's size, without the room around it for the shadow.
    body: (i32, i32),
    /// The window that had the focus, to give it back when nothing is chosen.
    previous: isize,
}

thread_local! {
    static MENU: std::cell::RefCell<Option<MenuState>> = const { std::cell::RefCell::new(None) };
}

/// Sizes of a menu at a scale, in pixels.
struct Metrics {
    margin: i32,
    pad: i32,
    row: i32,
    radius: f32,
    pill: f32,
}

fn metrics(scale: f32) -> Metrics {
    let px = |value: f32| (value * scale).round() as i32;
    Metrics {
        margin: px(14.0),
        pad: px(4.0),
        row: px(40.0),
        radius: 12.0 * scale,
        pill: 8.0 * scale,
    }
}

/// Signed distance from a point to a rounded rectangle: negative inside.
fn rounded(px: f32, py: f32, rect: (f32, f32, f32, f32), radius: f32) -> f32 {
    let (cx, cy) = ((rect.0 + rect.2) / 2.0, (rect.1 + rect.3) / 2.0);
    let (hx, hy) = (
        (rect.2 - rect.0) / 2.0 - radius,
        (rect.3 - rect.1) / 2.0 - radius,
    );
    let (qx, qy) = ((px - cx).abs() - hx, (py - cy).abs() - hy);
    (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - radius
}

/// How much of a pixel a shape with this distance covers.
fn coverage(distance: f32) -> f32 {
    (0.5 - distance).clamp(0.0, 1.0)
}

fn mix(a: [f32; 3], b: [f32; 3], amount: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * amount,
        a[1] + (b[1] - a[1]) * amount,
        a[2] + (b[2] - a[2]) * amount,
    ]
}

fn float3(rgb: [u8; 3]) -> [f32; 3] {
    [f32::from(rgb[0]), f32::from(rgb[1]), f32::from(rgb[2])]
}

/// 0x00bbggrr, as GDI takes a colour.
fn gdi(rgb: [u8; 3]) -> u32 {
    u32::from(rgb[0]) | (u32::from(rgb[1]) << 8) | (u32::from(rgb[2]) << 16)
}

fn rgb_of(hex: &str) -> [u8; 3] {
    let value = colorref(hex);
    [value as u8, (value >> 8) as u8, (value >> 16) as u8]
}

/// Whether apps are set to dark mode.
fn dark_mode() -> bool {
    let mut value: u32 = 1;
    let mut size = 4u32;
    let key = wide(r"Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
    let name = wide("AppsUseLightTheme");
    // SAFETY: reads one DWORD into `value`, whose size is given.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut value as *mut u32).cast(),
            &mut size,
        )
    };
    status == 0 && value == 0
}

impl MenuState {
    fn width_and_height(&self) -> (i32, i32) {
        let m = metrics(self.scale);
        (self.body.0 + 2 * m.margin, self.body.1 + 2 * m.margin)
    }

    /// The row under a point of the window, if any.
    fn row_at(&self, x: i32, y: i32) -> Option<usize> {
        let m = metrics(self.scale);
        let (left, top) = (m.margin + m.pad, m.margin + m.pad);
        if x < left || x >= m.margin + self.body.0 - m.pad || y < top {
            return None;
        }
        let row = ((y - top) / m.row) as usize;
        (row < self.rows.len()).then_some(row)
    }
}

/// Draws the menu into its layered window: the body and highlights by
/// distance to their shapes, then icons and text, then the alpha of the
/// rounded outline and its shadow.
unsafe fn render_menu(menu: &MenuState) {
    let scale = menu.scale;
    let m = metrics(scale);
    let px = |value: f32| (value * scale).round() as i32;
    let (bw, bh) = menu.body;
    let (w, h) = menu.width_and_height();
    let screen = GetDC(std::ptr::null_mut());
    let dc = CreateCompatibleDC(screen);
    let mut header: BITMAPINFO = std::mem::zeroed();
    header.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    header.bmiHeader.biWidth = w;
    header.bmiHeader.biHeight = -h;
    header.bmiHeader.biPlanes = 1;
    header.bmiHeader.biBitCount = 32;
    header.bmiHeader.biCompression = BI_RGB;
    let mut bits: *mut c_void = std::ptr::null_mut();
    let dib = CreateDIBSection(
        dc,
        &header,
        DIB_RGB_COLORS,
        &mut bits,
        std::ptr::null_mut(),
        0,
    );
    let before = SelectObject(dc, dib as _);
    let pixels = std::slice::from_raw_parts_mut(bits as *mut u32, (w * h) as usize);

    let (background, border, text, grey) = if menu.dark {
        ([43, 45, 49], [66, 69, 76], [242, 243, 245], [160, 164, 171])
    } else {
        (
            [255, 255, 255],
            [218, 220, 224],
            [27, 27, 27],
            [107, 111, 118],
        )
    };
    let accent = float3(menu.accent);
    let background_f = float3(background);
    let hover_tint = mix(background_f, accent, if menu.dark { 0.24 } else { 0.16 });
    let (fx, fy) = (m.margin as f32, m.margin as f32);
    let body = (fx, fy, fx + bw as f32, fy + bh as f32);
    let rows_top = fy + m.pad as f32;
    let (row_left, row_right) = (fx + m.pad as f32, fx + (bw - m.pad) as f32);
    let icon = px(22.0);
    let icon_left = row_left + px(10.0) as f32;

    // The body, its border, the hovered row, the marks of the shown row.
    for y in 0..h {
        for x in 0..w {
            let (cx, cy) = (x as f32 + 0.5, y as f32 + 0.5);
            let distance = rounded(cx, cy, body, m.radius);
            if distance > 1.0 {
                pixels[(y * w + x) as usize] = 0;
                continue;
            }
            let mut color = background_f;
            color = mix(
                color,
                float3(border),
                (distance + scale.max(1.0) + 0.5).clamp(0.0, 1.0),
            );
            for (i, row) in menu.rows.iter().enumerate() {
                let top = rows_top + (i as i32 * m.row) as f32;
                let bottom = top + m.row as f32;
                if cy < top - 2.0 || cy > bottom + 2.0 {
                    continue;
                }
                if menu.hover == Some(i) {
                    let pill = rounded(cx, cy, (row_left, top, row_right, bottom), m.pill);
                    color = mix(color, hover_tint, coverage(pill));
                }
                if row.checked {
                    let bar = (
                        row_left + 2.0 * scale,
                        top + 11.0 * scale,
                        row_left + 4.0 * scale,
                        bottom - 11.0 * scale,
                    );
                    color = mix(color, accent, coverage(rounded(cx, cy, bar, 1.0 * scale)));
                }
                if row.icon == 0 {
                    let top_icon = top + (m.row - icon) as f32 / 2.0;
                    let tile = (
                        icon_left,
                        top_icon,
                        icon_left + icon as f32,
                        top_icon + icon as f32,
                    );
                    let soft = mix(background_f, accent, 0.35);
                    color = mix(color, soft, coverage(rounded(cx, cy, tile, 6.0 * scale)));
                }
            }
            pixels[(y * w + x) as usize] = 0xff00_0000
                | ((color[0].round() as u32) << 16)
                | ((color[1].round() as u32) << 8)
                | color[2].round() as u32;
        }
    }

    // Icons and text, on the opaque body.
    SetBkMode(dc, TRANSPARENT as i32);
    for (i, row) in menu.rows.iter().enumerate() {
        let top = m.margin + m.pad + i as i32 * m.row;
        let icon_x = icon_left as i32;
        let icon_y = top + (m.row - icon) / 2;
        if row.icon != 0 {
            DrawIconEx(
                dc,
                icon_x,
                icon_y,
                row.icon as _,
                icon,
                icon,
                0,
                std::ptr::null_mut(),
                DI_NORMAL,
            );
        } else {
            let mut square = RECT {
                left: icon_x,
                top: icon_y,
                right: icon_x + icon,
                bottom: icon_y + icon,
            };
            let font = menu_font(px(12.0), true);
            let old = SelectObject(dc, font as _);
            SetTextColor(dc, gdi(text));
            let letter = wide(&row.letter.to_string());
            DrawTextW(
                dc,
                letter.as_ptr(),
                -1,
                &mut square,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE,
            );
            SelectObject(dc, old);
            DeleteObject(font as _);
        }
        let left = icon_x + icon + px(10.0);
        let right = m.margin + bw - m.pad - px(10.0);
        let block = px(32.0);
        let line_top = top + (m.row - block) / 2;
        let mut title = RECT {
            left,
            top: line_top,
            right,
            bottom: line_top + px(17.0),
        };
        let mut detail = RECT {
            left,
            top: line_top + px(17.0),
            right,
            bottom: line_top + block,
        };
        let flags = DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS | DT_VCENTER;
        if row.detail.is_empty() && !row.checked {
            // Nothing to say about the program: the title takes the row.
            title.bottom = line_top + block;
        }
        let font = menu_font(px(13.0), row.checked);
        let old = SelectObject(dc, font as _);
        SetTextColor(dc, gdi(text));
        let line = wide(&row.title);
        DrawTextW(dc, line.as_ptr(), -1, &mut title, flags);
        SelectObject(dc, old);
        DeleteObject(font as _);
        let font = menu_font(px(11.0), false);
        let old = SelectObject(dc, font as _);
        SetTextColor(dc, gdi(grey));
        let caption = if row.checked {
            format!("{} · showing", row.detail)
        } else {
            row.detail.clone()
        };
        let line = wide(&caption);
        DrawTextW(dc, line.as_ptr(), -1, &mut detail, flags);
        SelectObject(dc, old);
        DeleteObject(font as _);
    }

    // The outline's alpha, and the shadow beneath it.
    let shadow = (
        fx,
        fy + 4.0 * scale,
        fx + bw as f32,
        fy + bh as f32 + 4.0 * scale,
    );
    let blur = m.margin as f32;
    for y in 0..h {
        for x in 0..w {
            let (cx, cy) = (x as f32 + 0.5, y as f32 + 0.5);
            let cover = coverage(rounded(cx, cy, body, m.radius));
            let away = rounded(cx, cy, shadow, m.radius).max(0.0);
            let shade = 0.34 * (1.0 - away / blur).clamp(0.0, 1.0).powi(2);
            let alpha = cover + shade * (1.0 - cover);
            let pixel = pixels[(y * w + x) as usize];
            let channel = |shift: u32| (((pixel >> shift) & 0xff) as f32 * cover).round() as u32;
            pixels[(y * w + x) as usize] = ((alpha * 255.0).round() as u32) << 24
                | channel(16) << 16
                | channel(8) << 8
                | channel(0);
        }
    }
    let size = SIZE { cx: w, cy: h };
    let from = POINT { x: 0, y: 0 };
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    UpdateLayeredWindow(
        menu.hwnd as HWND,
        screen,
        std::ptr::null(),
        &size,
        dc,
        &from,
        0,
        &blend,
        ULW_ALPHA,
    );
    SelectObject(dc, before);
    DeleteObject(dib as _);
    DeleteDC(dc);
    ReleaseDC(std::ptr::null_mut(), screen);
}

/// Closes the menu and reports the row chosen, or nothing when it was dismissed.
fn close_menu(choice: Option<usize>) {
    let Some(menu) = MENU.with(|slot| slot.borrow_mut().take()) else {
        return;
    };
    unsafe {
        ReleaseCapture();
        DestroyWindow(menu.hwnd as HWND);
    }
    match choice.and_then(|row| menu.ids.get(row)) {
        Some(id) => note(Note::Menu(menu.key, Some(*id))),
        None => {
            if menu.previous != 0 {
                unsafe { SetForegroundWindow(menu.previous as HWND) };
            }
            note(Note::Menu(menu.key, None));
        }
    }
}

/// Moves the highlight and draws again.
fn set_hover(hover: Option<usize>) {
    MENU.with(|slot| {
        if let Some(menu) = slot.borrow_mut().as_mut() {
            if menu.hover != hover {
                menu.hover = hover;
                unsafe { render_menu(menu) };
            }
        }
    });
}

unsafe extern "system" fn menu_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let point = || (i32::from(lparam as i16), i32::from((lparam >> 16) as i16));
    match msg {
        WM_MOUSEMOVE => {
            let (x, y) = point();
            let row = MENU.with(|slot| slot.borrow().as_ref().and_then(|menu| menu.row_at(x, y)));
            set_hover(row);
            0
        }
        WM_LBUTTONUP | WM_LBUTTONDOWN | WM_RBUTTONDOWN => {
            let (x, y) = point();
            let (row, inside) = MENU.with(|slot| {
                slot.borrow().as_ref().map_or((None, false), |menu| {
                    let (w, h) = menu.width_and_height();
                    (menu.row_at(x, y), x >= 0 && y >= 0 && x < w && y < h)
                })
            });
            if msg == WM_LBUTTONUP && row.is_some() {
                close_menu(row);
            } else if !inside {
                // The mouse is captured: a click outside dismisses.
                close_menu(None);
            }
            0
        }
        WM_KEYDOWN => {
            let key = wparam as u16;
            let (count, hover) = MENU.with(|slot| {
                slot.borrow()
                    .as_ref()
                    .map_or((0, None), |menu| (menu.rows.len(), menu.hover))
            });
            if key == VK_ESCAPE {
                close_menu(None);
            } else if key == VK_RETURN {
                if hover.is_some() {
                    close_menu(hover);
                }
            } else if key == VK_DOWN && count > 0 {
                set_hover(Some(hover.map_or(0, |row| (row + 1) % count)));
            } else if key == VK_UP && count > 0 {
                set_hover(Some(
                    hover.map_or(count - 1, |row| (row + count - 1) % count),
                ));
            }
            0
        }
        WM_ACTIVATE => {
            if (wparam & 0xffff) as u32 == WA_INACTIVE {
                close_menu(None);
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Opens the menu at `at`, kept inside the screen, and returns at once: the
/// outcome comes back as a [`Note::Menu`] when the user chooses or dismisses.
fn open_menu(key: u32, items: &[MenuItem], at: Point, accent: &str) {
    close_menu(None);
    unsafe {
        let scale = ui_scale();
        let m = metrics(scale);
        let px = |value: f32| (value * scale).round() as i32;
        let rows: Vec<MenuRow> = items
            .iter()
            .map(|item| {
                let window = item.window.map(|id| id as HWND);
                MenuRow {
                    title: item.label.clone(),
                    detail: window.map(|w| program_name(w)).unwrap_or_default(),
                    checked: item.checked,
                    icon: window.map(|w| window_icon(w)).unwrap_or(0),
                    letter: item
                        .label
                        .chars()
                        .next()
                        .unwrap_or('?')
                        .to_uppercase()
                        .next()
                        .unwrap_or('?'),
                }
            })
            .collect();
        if rows.is_empty() {
            note(Note::Menu(key, None));
            return;
        }
        let widest = rows
            .iter()
            .map(|row| title_width(&row.title, row.checked, scale))
            .max()
            .unwrap_or(0);
        let body_w =
            (widest + px(10.0 + 22.0 + 10.0 + 10.0) + 2 * m.pad).clamp(px(230.0), px(420.0));
        let body_h = 2 * m.pad + rows.len() as i32 * m.row;
        // Beside the pointer, but inside the screen it is on.
        let monitor = MonitorFromPoint(POINT { x: at.x, y: at.y }, MONITOR_DEFAULTTONEAREST);
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(monitor, &mut info);
        let work = info.rcWork;
        let x = at.x.clamp(work.left, (work.right - body_w).max(work.left));
        let y = if at.y + body_h > work.bottom {
            (at.y - body_h - px(36.0)).max(work.top)
        } else {
            at.y
        };
        let state = MenuState {
            hwnd: 0,
            key,
            ids: items.iter().map(|item| item.id).collect(),
            rows,
            hover: None,
            accent: rgb_of(accent),
            dark: dark_mode(),
            scale,
            body: (body_w, body_h),
            previous: GetForegroundWindow() as isize,
        };
        let (w, h) = state.width_and_height();
        let class = wide("HomeFeelingMenu");
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            class.as_ptr(),
            std::ptr::null(),
            WS_POPUP,
            x - m.margin,
            y - m.margin,
            w,
            h,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            GetModuleHandleW(std::ptr::null()),
            std::ptr::null(),
        );
        if hwnd.is_null() {
            note(Note::Menu(key, None));
            return;
        }
        let state = MenuState {
            hwnd: hwnd as isize,
            ..state
        };
        render_menu(&state);
        MENU.with(|slot| *slot.borrow_mut() = Some(state));
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
        SetFocus(hwnd);
        // Every click, inside or out, comes here: an outside one dismisses.
        SetCapture(hwnd);
    }
}

fn clear_overlay(key: u32) {
    if let Some(edges) = OVERLAYS.with(|o| o.borrow_mut().remove(&key)) {
        for hwnd in edges {
            COLORS.with(|c| c.borrow_mut().remove(&hwnd));
            BADGES.with(|b| b.borrow_mut().remove(&hwnd));
            SMOOTH.with(|m| m.borrow_mut().remove(&hwnd));
            unsafe { DestroyWindow(hwnd as HWND) };
        }
    }
}

/// Reads the keyboard right now: the hotkey being held, if a key with at
/// least one modifier is down. Escape and Delete or Backspace are answers of
/// their own.
pub(crate) fn capture() -> crate::Capture {
    use crate::Capture;
    let down = |vk: u16| unsafe { (GetAsyncKeyState(i32::from(vk)) as u16) & 0x8000 != 0 };
    if down(VK_ESCAPE) {
        return Capture::Cancelled;
    }
    if down(VK_DELETE) || down(VK_BACK) {
        return Capture::Cleared;
    }
    let modifiers = crate::Modifiers {
        alt: down(VK_MENU),
        ctrl: down(VK_CONTROL),
        shift: down(VK_SHIFT),
        win: down(VK_LWIN) || down(VK_RWIN),
    };
    // Caps Lock is a hotkey on its own, and a hotkey that owns it hides it
    // from the keyboard state, so the hook's last press counts too.
    let tick = CAPS_TICK.load(std::sync::atomic::Ordering::Relaxed);
    let recent = tick != 0 && unsafe { GetTickCount() }.wrapping_sub(tick) < 250;
    if down(VK_CAPITAL) || recent {
        return Capture::Combo(
            Hotkey {
                modifiers,
                key: Key::CapsLock,
            }
            .to_string(),
        );
    }
    if !(modifiers.alt || modifiers.ctrl || modifiers.shift || modifiers.win) {
        return Capture::Waiting;
    }
    let mut keys = vec![
        (VK_LEFT, Key::Left),
        (VK_RIGHT, Key::Right),
        (VK_UP, Key::Up),
        (VK_DOWN, Key::Down),
        (VK_SPACE, Key::Space),
        (VK_RETURN, Key::Enter),
        (VK_TAB, Key::Tab),
    ];
    keys.extend((b'A'..=b'Z').map(|c| (u16::from(c), Key::Letter(c as char))));
    keys.extend((b'0'..=b'9').map(|c| (u16::from(c), Key::Digit(c as char))));
    keys.extend((1..=12u8).map(|n| (VK_F1 + u16::from(n) - 1, Key::Function(n))));
    match keys.into_iter().find(|(vk, _)| down(*vk)) {
        Some((_, key)) => Capture::Combo(Hotkey { modifiers, key }.to_string()),
        None => Capture::Waiting,
    }
}

/// When Caps Lock was last pressed, by tick count; the key never reaches
/// the keyboard state while a hotkey owns it, so the recorder asks here.
static CAPS_TICK: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Takes every Caps Lock event while a hotkey owns the key: a press fires
/// the hotkeys whose modifiers are held, and nothing reaches the keyboard
/// state, so capitals never turn on.
unsafe extern "system" fn caps_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let info = &*(lparam as *const KBDLLHOOKSTRUCT);
        // Keys this program sends itself, to switch capitals off, pass.
        if info.vkCode == u32::from(VK_CAPITAL) && info.flags & LLKHF_INJECTED == 0 {
            if wparam == WM_KEYDOWN as usize || wparam == WM_SYSKEYDOWN as usize {
                // Holding the key repeats it; one press fires once.
                if !CAPS_DOWN.with(|down| down.replace(true)) {
                    CAPS_TICK.store(GetTickCount().max(1), std::sync::atomic::Ordering::Relaxed);
                    let held = |vk: u16| (GetAsyncKeyState(i32::from(vk)) as u16) & 0x8000 != 0;
                    let now = Modifiers {
                        alt: held(VK_MENU),
                        ctrl: held(VK_CONTROL),
                        shift: held(VK_SHIFT),
                        win: held(VK_LWIN) || held(VK_RWIN),
                    };
                    for (key, modifiers) in CAPS.with(|caps| caps.borrow().clone()) {
                        if modifiers == now {
                            note(Note::Hotkey(key));
                        }
                    }
                }
            } else {
                CAPS_DOWN.with(|down| down.set(false));
            }
            return 1;
        }
    }
    CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
}

/// Gives Caps Lock to a hotkey, and switches capitals off if they are on.
fn register_caps(key: u32, modifiers: Modifiers) -> Result<(), String> {
    unsafe {
        if CAPS_HOOK.with(|hook| hook.get()) == 0 {
            if GetKeyState(i32::from(VK_CAPITAL)) & 1 != 0 {
                keybd_event(VK_CAPITAL as u8, 0, 0, 0);
                keybd_event(VK_CAPITAL as u8, 0, KEYEVENTF_KEYUP, 0);
            }
            let hook = SetWindowsHookExW(
                WH_KEYBOARD_LL,
                Some(caps_hook),
                GetModuleHandleW(std::ptr::null()),
                0,
            );
            if hook.is_null() {
                return Err("could not take over Caps Lock".into());
            }
            CAPS_HOOK.with(|slot| slot.set(hook as isize));
        }
    }
    CAPS.with(|caps| caps.borrow_mut().push((key, modifiers)));
    Ok(())
}

/// Lets go of a Caps Lock hotkey, and of the key when none is left. False
/// when `key` was not one.
fn unregister_caps(key: u32) -> bool {
    let was = CAPS.with(|caps| {
        let mut caps = caps.borrow_mut();
        let before = caps.len();
        caps.retain(|(k, _)| *k != key);
        before != caps.len()
    });
    if was && CAPS.with(|caps| caps.borrow().is_empty()) {
        let hook = CAPS_HOOK.with(|slot| slot.replace(0));
        if hook != 0 {
            unsafe { UnhookWindowsHookEx(hook as HHOOK) };
        }
        CAPS_DOWN.with(|down| down.set(false));
    }
    was
}

fn register(key: u32, hotkey: Hotkey) -> Result<(), String> {
    let m = hotkey.modifiers;
    let mut mods = MOD_NOREPEAT;
    if m.alt {
        mods |= MOD_ALT;
    }
    if m.ctrl {
        mods |= MOD_CONTROL;
    }
    if m.shift {
        mods |= MOD_SHIFT;
    }
    if m.win {
        mods |= MOD_WIN;
    }
    let vk = match hotkey.key {
        Key::Letter(c) | Key::Digit(c) => c as u32,
        Key::Left => VK_LEFT as u32,
        Key::Right => VK_RIGHT as u32,
        Key::Up => VK_UP as u32,
        Key::Down => VK_DOWN as u32,
        Key::Space => VK_SPACE as u32,
        Key::Enter => VK_RETURN as u32,
        Key::Tab => VK_TAB as u32,
        Key::Function(n) => VK_F1 as u32 + u32::from(n) - 1,
        Key::CapsLock => return register_caps(key, hotkey.modifiers),
    };
    if unsafe { RegisterHotKey(std::ptr::null_mut(), key as i32, mods, vk) } == 0 {
        return Err("already taken by Windows or another program".into());
    }
    Ok(())
}

fn event_thread(commands: Receiver<Command>, id: Sender<u32>) {
    unsafe {
        let class = wide("HomeFeelingOverlay");
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(overlay_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: GetModuleHandleW(std::ptr::null()),
            hIcon: std::ptr::null_mut(),
            hCursor: std::ptr::null_mut(),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class.as_ptr(),
        };
        RegisterClassW(&wc);
        let menu_class = wide("HomeFeelingMenu");
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(menu_proc),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            lpszClassName: menu_class.as_ptr(),
            ..wc
        });
        // A hidden top-level window hears display changes.
        let _display = create_overlay_window(&class);
        SetWinEventHook(
            EVENT_MIN,
            EVENT_MAX,
            std::ptr::null_mut(),
            Some(on_event),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        );
        let _ = id.send(GetCurrentThreadId());
        let mut registered: HashSet<u32> = HashSet::new();
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            match msg.message {
                WM_HOTKEY => note(Note::Hotkey(msg.wParam as u32)),
                WM_COMMANDS => {
                    while let Ok(command) = commands.try_recv() {
                        match command {
                            Command::RegisterHotkey(key, hotkey, reply) => {
                                let result = register(key, hotkey);
                                if result.is_ok() {
                                    registered.insert(key);
                                }
                                let _ = reply.send(result);
                            }
                            Command::UnregisterHotkey(key) => {
                                if registered.remove(&key) && !unregister_caps(key) {
                                    UnregisterHotKey(std::ptr::null_mut(), key as i32);
                                }
                            }
                            Command::SetOverlay(key, overlay) => set_overlay(&class, key, &overlay),
                            Command::ClearOverlay(key) => clear_overlay(key),
                            Command::ShowMenu(key, items, at, accent) => {
                                open_menu(key, &items, at, &accent)
                            }
                        }
                    }
                }
                _ => {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }
    }
}

#[cfg(test)]
mod live {
    use super::*;

    /// Touches the real desktop: run with `cargo test -p home-feeling-desktop -- --ignored`.
    /// Opens the badge menu and keeps it up for a few seconds, for a screenshot.
    #[test]
    #[ignore]
    fn shows_a_popup_menu() {
        let mut windows = Windows::start();
        let items = vec![
            MenuItem {
                id: 1,
                label: "Calendar | Microsoft Teams".into(),
                checked: false,
                window: None,
            },
            MenuItem {
                id: 2,
                label: "WG-10 Handling and releasing vibe-coded products - 2N OS - Confluence - Google Chrome".into(),
                checked: true,
                window: None,
            },
        ];
        windows.show_menu(1, &items, Point { x: 300, y: 300 }, "#00bcd4");
        std::thread::sleep(std::time::Duration::from_secs(7));
    }

    #[test]
    #[ignore]
    fn sees_monitors_and_windows_and_draws_an_overlay() {
        let mut windows = Windows::start();
        let snapshot = windows.snapshot();
        assert!(!snapshot.monitors.is_empty());
        println!(
            "{} monitors, {} windows",
            snapshot.monitors.len(),
            snapshot.windows.len()
        );
        for window in snapshot.windows.iter().take(5) {
            println!(
                "  window {:?} {:?} {:?}",
                window.title, window.class, window.rect
            );
        }
        let work = snapshot.monitors[0].work;
        let overlay = Overlay {
            rect: Rect {
                x: work.x + 100,
                y: work.y + 100,
                w: 200,
                h: 100,
            },
            color: "#ff0000".into(),
            width: 4,
            fill: Some(0.25),
            radius: 12,
            badge: None,
            attach: None,
        };
        windows.set_overlay(1, &overlay);
        windows
            .register_hotkey(1, Hotkey::parse("ctrl+alt+shift+f12").unwrap())
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        windows.clear_overlay(1);
        windows.unregister_hotkey(1);
        let _ = windows.poll();
    }
}
