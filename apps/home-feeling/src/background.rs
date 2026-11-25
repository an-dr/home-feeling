//! Keeping the app alive in the tray: closing the window hides it instead of
//! ending the program, and the tray brings it back or quits for real.
//!
//! bones turns a window close into a quit before any module hears of it, so on
//! Windows the window procedure is wrapped to take `WM_CLOSE` first; the hide
//! itself goes through SDL, which then knows the window is hidden and shows it
//! again when asked. Everything here runs on the frame thread, the one SDL's
//! windows belong to. On Linux there is no tray, so closing still quits.
use std::sync::atomic::{AtomicBool, Ordering};

use sdl3::sys::video::{
    SDL_GetWindows, SDL_HideWindow, SDL_RaiseWindow, SDL_RestoreWindow, SDL_ShowWindow, SDL_Window,
};

/// A close was asked for and has not been answered by hiding yet.
static CLOSE_ASKED: AtomicBool = AtomicBool::new(false);
/// The user chose Quit: a close now really closes.
static ALLOW_CLOSE: AtomicBool = AtomicBool::new(false);

pub struct Background {
    installed: bool,
    /// Hide the window as soon as it exists: the app was started to sit in the tray.
    start_hidden: bool,
}

impl Background {
    pub fn new(start_hidden: bool) -> Self {
        Self {
            installed: false,
            start_hidden,
        }
    }

    /// Called every frame: takes over the window's close, and hides the window
    /// when a close was asked for or the app should start in the tray.
    pub fn tick(&mut self) {
        let Some(window) = window() else {
            return;
        };
        if !self.installed {
            self.installed = install(window);
            if self.start_hidden {
                // SAFETY: `window` came from SDL this frame, on the frame thread.
                unsafe { SDL_HideWindow(window) };
            }
        }
        if CLOSE_ASKED.swap(false, Ordering::SeqCst) {
            // SAFETY: as above.
            unsafe { SDL_HideWindow(window) };
        }
    }

    /// Shows the window again, in front.
    pub fn show(&self) {
        if let Some(window) = window() {
            // SAFETY: `window` came from SDL just now, on the frame thread.
            unsafe {
                SDL_ShowWindow(window);
                SDL_RestoreWindow(window);
                SDL_RaiseWindow(window);
            }
        }
    }

    /// The next close really closes: the user chose Quit.
    pub fn allow_close(&self) {
        ALLOW_CLOSE.store(true, Ordering::SeqCst);
    }
}

/// The app's one window, if SDL has it.
fn window() -> Option<*mut SDL_Window> {
    // SAFETY: the list is SDL's, freed after its first entry is read.
    unsafe {
        let mut count = 0;
        let list = SDL_GetWindows(&mut count);
        if list.is_null() {
            return None;
        }
        let first = if count > 0 {
            *list
        } else {
            std::ptr::null_mut()
        };
        sdl3::sys::stdinc::SDL_free(list.cast());
        (!first.is_null()).then_some(first)
    }
}

#[cfg(windows)]
mod imp {
    use std::sync::atomic::{AtomicIsize, Ordering};

    use sdl3::sys::properties::SDL_GetPointerProperty;
    use sdl3::sys::video::{
        SDL_GetWindowProperties, SDL_Window, SDL_PROP_WINDOW_WIN32_HWND_POINTER,
    };
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallWindowProcW, SetWindowLongPtrW, GWLP_WNDPROC, WM_CLOSE, WNDPROC,
    };

    use super::{ALLOW_CLOSE, CLOSE_ASKED};
    use std::sync::atomic::Ordering as Order;

    /// SDL's own window procedure, which the wrapper hands everything else to.
    static ORIGINAL: AtomicIsize = AtomicIsize::new(0);

    unsafe extern "system" fn wrapper(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if msg == WM_CLOSE && !ALLOW_CLOSE.load(Order::SeqCst) {
            // Not a quit: the frame loop hides the window on its next turn.
            CLOSE_ASKED.store(true, Order::SeqCst);
            return 0;
        }
        let original: WNDPROC = std::mem::transmute(ORIGINAL.load(Ordering::SeqCst));
        CallWindowProcW(original, hwnd, msg, wparam, lparam)
    }

    pub fn install(window: *mut SDL_Window) -> bool {
        // SAFETY: `window` is live SDL's, and the procedure is replaced from
        // the thread the window belongs to.
        unsafe {
            let hwnd = SDL_GetPointerProperty(
                SDL_GetWindowProperties(window),
                SDL_PROP_WINDOW_WIN32_HWND_POINTER,
                std::ptr::null_mut(),
            );
            if hwnd.is_null() {
                return false;
            }
            let old = SetWindowLongPtrW(hwnd as HWND, GWLP_WNDPROC, wrapper as *const () as isize);
            ORIGINAL.store(old, Ordering::SeqCst);
            old != 0
        }
    }
}

#[cfg(windows)]
use imp::install;

/// Without a tray the window's close stays a quit.
#[cfg(not(windows))]
fn install(_: *mut SDL_Window) -> bool {
    true
}
