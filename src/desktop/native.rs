//! The one place Context reaches past GPUI to the operating system.
//!
//! GPUI 0.2 shows pop-up windows on Windows by activating them, which would take
//! keyboard focus from the app the user is typing in. On Windows the card is
//! created hidden, then placed and shown here as a topmost window that never
//! activates. macOS and Linux pop-ups already behave that way, so this is a
//! no-op there. Remove this module once GPUI supports non-activating pop-ups on
//! Windows.

use gpui::{Bounds, Pixels, Window};

/// Whether GPUI should show the card itself when it opens.
pub const SHOW_CARD_ON_OPEN: bool = !cfg!(windows);

/// A pending native placement for the card.
pub struct Float(imp::Float);

/// Reads what the native call needs. Call inside a GPUI window update.
pub fn prepare_float(window: &Window, bounds: Bounds<Pixels>) -> crate::Result<Float> {
    imp::prepare(window, bounds).map(Float)
}

impl Float {
    /// Shows the card above other windows without taking focus. Call outside
    /// any GPUI update: the native resize notifies GPUI, which needs the app.
    pub fn apply(self) -> crate::Result<()> {
        imp::apply(self.0)
    }
}

#[cfg(windows)]
mod imp {
    use gpui::{Bounds, Pixels, Window};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetWindowLongPtrW, HWND_TOPMOST, SWP_NOACTIVATE, SWP_SHOWWINDOW,
        SetWindowLongPtrW, SetWindowPos, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    };

    pub struct Float {
        hwnd: HWND,
        rect: [i32; 4],
    }

    pub fn prepare(window: &Window, bounds: Bounds<Pixels>) -> crate::Result<Float> {
        let handle = HasWindowHandle::window_handle(window).map_err(|error| error.to_string())?;
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return Err("The card is not a Win32 window.".into());
        };
        // GPUI bounds are logical pixels; Win32 wants device pixels.
        let scale = window.scale_factor();
        let device = |value: Pixels| (f32::from(value) * scale).round() as i32;
        Ok(Float {
            hwnd: HWND(handle.hwnd.get() as *mut _),
            rect: [
                device(bounds.origin.x),
                device(bounds.origin.y),
                device(bounds.size.width),
                device(bounds.size.height),
            ],
        })
    }

    pub fn apply(float: Float) -> crate::Result<()> {
        let [x, y, width, height] = float.rect;
        // SAFETY: `hwnd` is a GPUI window created on this thread. A window that
        // closed in the meantime makes these calls fail, which is reported.
        unsafe {
            let style = GetWindowLongPtrW(float.hwnd, GWL_EXSTYLE);
            SetWindowLongPtrW(
                float.hwnd,
                GWL_EXSTYLE,
                style | (WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW).0 as isize,
            );
            SetWindowPos(
                float.hwnd,
                Some(HWND_TOPMOST),
                x,
                y,
                width,
                height,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            )?;
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod imp {
    use gpui::{Bounds, Pixels, Window};

    pub struct Float;

    pub fn prepare(_window: &Window, _bounds: Bounds<Pixels>) -> crate::Result<Float> {
        Ok(Float)
    }

    pub fn apply(_float: Float) -> crate::Result<()> {
        Ok(())
    }
}
