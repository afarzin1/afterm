//! The Windows title bar, painted in the colour scheme's own colours.
//!
//! afterm draws no title bar of its own — the native one is kept, for snap
//! layouts, the real window buttons and every resize behaviour that comes with
//! it — but a stock white caption above a dark window is the one seam left in
//! a design that is otherwise one surface. DWM will paint the caption for us,
//! so the seam closes without giving up anything native.
//!
//! Windows 11 only, for the colour: `DWMWA_CAPTION_COLOR` arrived in build
//! 22000 and older builds reject the attribute. That is why the window also
//! asks for the dark theme in `tauri.conf.json`, which Windows 10 does honour —
//! there the caption is dark rather than exact, and it is never white.

#[cfg(windows)]
use tauri::WebviewWindow;

/// A `#rrggbb` string as a plain RGB integer.
///
/// Outside Windows nothing calls this but its own tests, and they are worth
/// keeping there: parsing what the stylesheet hands over is the only part of
/// this module that can be wrong on a machine that can run the test.
#[cfg_attr(not(windows), allow(dead_code))]
fn rgb(colour: &str) -> Option<u32> {
    let digits = colour.strip_prefix('#').unwrap_or(colour);
    if digits.len() != 6 {
        return None;
    }
    u32::from_str_radix(digits, 16).ok()
}

/// Paint the caption, its border and its text.
///
/// Best-effort by design: an unsupported attribute is how Windows 10 answers,
/// and a window with the wrong caption colour is not worth failing a command
/// over — let alone one the interface calls on every theme change.
#[cfg(windows)]
pub fn apply(window: &WebviewWindow, caption: &str, text: &str) {
    use std::ffi::c_void;
    use windows::Win32::Foundation::COLORREF;
    use windows::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR,
        DWMWINDOWATTRIBUTE,
    };

    let (Some(caption), Some(text)) = (rgb(caption), rgb(text)) else {
        return;
    };
    let Ok(handle) = window.hwnd() else {
        return;
    };

    // COLORREF is 0x00bbggrr, the reverse of the 0xrrggbb everything else here
    // uses.
    let colorref = |value: u32| {
        COLORREF(((value & 0xff) << 16) | (value & 0xff00) | ((value >> 16) & 0xff))
    };

    let set = |attribute: DWMWINDOWATTRIBUTE, value: COLORREF| unsafe {
        let _ = DwmSetWindowAttribute(
            handle,
            attribute,
            &value as *const COLORREF as *const c_void,
            std::mem::size_of::<COLORREF>() as u32,
        );
    };

    set(DWMWA_CAPTION_COLOR, colorref(caption));
    set(DWMWA_BORDER_COLOR, colorref(caption));
    set(DWMWA_TEXT_COLOR, colorref(text));
}

#[cfg(not(windows))]
pub fn apply(_window: &tauri::WebviewWindow, _caption: &str, _text: &str) {}

#[cfg(test)]
mod tests {
    use super::rgb;

    #[test]
    fn reads_the_colours_the_stylesheet_writes() {
        assert_eq!(rgb("#161b25"), Some(0x161b25));
        assert_eq!(rgb("161616"), Some(0x161616));
    }

    #[test]
    fn refuses_anything_else() {
        // A custom property that resolved to empty, a shorthand, or a value
        // carrying alpha. None of them are a caption colour.
        assert_eq!(rgb(""), None);
        assert_eq!(rgb("#fff"), None);
        assert_eq!(rgb("#1a202cdd"), None);
        assert_eq!(rgb("#nothex"), None);
    }
}
