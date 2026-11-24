//! The Windows backend. One thread owns the event hook, the hotkeys, and the
//! overlay windows, because Win32 ties each of them to the thread that made
//! it; the module talks to that thread through channels.
use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Mutex, OnceLock};

use windows_sys::core::BOOL;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS,
};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, EnumDisplayMonitors, FillRect,
    GetMonitorInfoW, InvalidateRect, MonitorFromWindow, HDC, HMONITOR, MONITORINFO,
    MONITOR_DEFAULTTONEAREST, PAINTSTRUCT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId};
use windows_sys::Win32::UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    keybd_event, RegisterHotKey, UnregisterHotKey, KEYEVENTF_KEYUP, MOD_ALT, MOD_CONTROL,
    MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, VK_DOWN, VK_F1, VK_LEFT, VK_MENU, VK_RETURN, VK_RIGHT,
    VK_SPACE, VK_TAB, VK_UP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::keys::{Hotkey, Key};
use crate::protocol::{Capabilities, Id, Monitor, Overlay, Point, Rect, Snapshot, WindowInfo};
use crate::{Backend, RawEvent};

/// Wakes the thread to read its command channel.
const WM_COMMANDS: u32 = WM_APP + 1;

enum Command {
    RegisterHotkey(u32, Hotkey, Sender<Result<(), String>>),
    UnregisterHotkey(u32),
    SetOverlay(u32, Overlay),
    ClearOverlay(u32),
}

/// Raw notifications from the hook and the message loop.
enum Note {
    Window(u32, isize),
    Hotkey(u32),
    Display,
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
    }
}

thread_local! {
    /// Each overlay's four edge windows and their colour, by key.
    static OVERLAYS: std::cell::RefCell<HashMap<u32, [isize; 4]>> = Default::default();
    static COLORS: std::cell::RefCell<HashMap<isize, u32>> = Default::default();
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

fn set_overlay(class: &[u16], key: u32, overlay: &Overlay) {
    let edges = OVERLAYS.with(|o| {
        *o.borrow_mut()
            .entry(key)
            .or_insert_with(|| std::array::from_fn(|_| create_overlay_window(class)))
    });
    let (r, g, b) = overlay.rgb();
    let color = u32::from(r) | (u32::from(g) << 8) | (u32::from(b) << 16);
    let Rect { x, y, w, h } = overlay.rect;
    let t = overlay.width.max(1);
    let places = [
        (x - t, y - t, w + 2 * t, t),
        (x - t, y + h, w + 2 * t, t),
        (x - t, y, t, h),
        (x + w, y, t, h),
    ];
    for (hwnd, (px, py, pw, ph)) in edges.iter().zip(places) {
        COLORS.with(|c| c.borrow_mut().insert(*hwnd, color));
        unsafe {
            SetWindowPos(
                *hwnd as HWND,
                HWND_TOPMOST,
                px,
                py,
                pw,
                ph,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            InvalidateRect(*hwnd as HWND, std::ptr::null(), 1);
        }
    }
}

fn clear_overlay(key: u32) {
    if let Some(edges) = OVERLAYS.with(|o| o.borrow_mut().remove(&key)) {
        for hwnd in edges {
            COLORS.with(|c| c.borrow_mut().remove(&hwnd));
            unsafe { DestroyWindow(hwnd as HWND) };
        }
    }
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
                                if registered.remove(&key) {
                                    UnregisterHotKey(std::ptr::null_mut(), key as i32);
                                }
                            }
                            Command::SetOverlay(key, overlay) => set_overlay(&class, key, &overlay),
                            Command::ClearOverlay(key) => clear_overlay(key),
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
