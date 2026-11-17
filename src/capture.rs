//! Screen capture through `xcap`, which handles Windows, macOS, X11 and Wayland.
//! On Wayland the desktop's screenshot portal may ask which screen to share.

use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, RgbaImage};
use xcap::{Monitor, Window};

use crate::Result;

/// Large captures are scaled to fit this edge without cropping.
const MAX_EDGE: u32 = 2200;
const JPEG_QUALITY: u8 = 85;

/// One capture of the display the user is working on.
pub struct Snapshot {
    /// The capture as JPEG. History keeps it with the result.
    pub jpeg: Vec<u8>,
    pub focus: Focus,
}

/// What the desktop reports about the active window. Some desktops, such as
/// Wayland compositors, report nothing.
#[derive(Clone, Default)]
pub struct Focus {
    pub app: Option<String>,
    pub title: Option<String>,
}

pub fn take() -> Result<Snapshot> {
    let window = focused_window();
    if window
        .as_ref()
        .is_some_and(|window| window.pid().ok() == Some(std::process::id()))
    {
        return Err("Switch to the app you want Context to look at, then try again.".into());
    }
    let monitor = match window.as_ref().and_then(|w| w.current_monitor().ok()) {
        Some(monitor) => monitor,
        None => primary_monitor()?,
    };
    let frame = monitor.capture_image().map_err(|_| {
        "Context could not capture the screen. Allow screen recording for Context in your \
         system settings, then try again."
    })?;
    let focus = Focus {
        app: window
            .as_ref()
            .and_then(|w| w.app_name().ok())
            .and_then(tidy_app_name),
        title: window
            .as_ref()
            .and_then(|w| w.title().ok())
            .map(|title| title.trim().to_owned())
            .filter(|title| !title.is_empty()),
    };
    Ok(Snapshot {
        jpeg: encode(DynamicImage::ImageRgba8(frame))?,
        focus,
    })
}

/// The focused window's own pixels, for automatic cards to compare and read
/// locally. `None` when Context itself is focused, the window is minimized, or
/// `skip` returns true for its app, which is then never captured.
pub struct WindowShot {
    pub app: String,
    pub title: String,
    pub image: RgbaImage,
}

pub fn focused_window_image(skip: impl Fn(&str) -> bool) -> Option<WindowShot> {
    let window = focused_window()?;
    if window.pid().ok() == Some(std::process::id()) || window.is_minimized().unwrap_or(false) {
        return None;
    }
    let app = window.app_name().ok().and_then(tidy_app_name)?;
    if skip(&app) {
        return None;
    }
    let title = window.title().unwrap_or_default();
    let image = window.capture_image().ok()?;
    Some(WindowShot { app, title, image })
}

fn focused_window() -> Option<Window> {
    Window::all()
        .ok()?
        .into_iter()
        .find(|window| window.is_focused().unwrap_or(false))
}

fn primary_monitor() -> Result<Monitor> {
    let monitors = Monitor::all()?;
    let index = monitors
        .iter()
        .position(|monitor| monitor.is_primary().unwrap_or(false))
        .unwrap_or(0);
    monitors
        .into_iter()
        .nth(index)
        .ok_or_else(|| "Context could not find a display to capture.".into())
}

fn tidy_app_name(name: String) -> Option<String> {
    let name = name.trim();
    let name = name
        .strip_suffix(".exe")
        .or_else(|| name.strip_suffix(".EXE"))
        .unwrap_or(name);
    (!name.is_empty()).then(|| name.to_owned())
}

fn encode(frame: DynamicImage) -> Result<Vec<u8>> {
    let frame = if frame.width().max(frame.height()) > MAX_EDGE {
        frame.thumbnail(MAX_EDGE, MAX_EDGE)
    } else {
        frame
    };
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, JPEG_QUALITY).encode_image(&frame.to_rgb8())?;
    Ok(jpeg)
}
