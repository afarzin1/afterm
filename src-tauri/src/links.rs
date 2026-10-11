//! Opening a link from the terminal in the default browser.
//!
//! Every link the terminal offers came from the far end: a URL a program
//! printed, or an OSC 8 hyperlink, whose target is whatever the host chose and
//! need not look anything like the text it is drawn on. So what is handed to
//! the operating system is decided here, not in the page, and the answer is
//! narrow on purpose: `http` and `https`, with a host, and nothing else.
//!
//! The other schemes are the danger, not the edge case. Windows resolves a URL
//! through whatever handler is registered for its scheme, and `file:`,
//! `search-ms:`, `ms-msdt:` and a long tail of vendor schemes have each been
//! the way a click became code running on this machine. A terminal has no
//! business passing any of them on.

use url::Url;

/// Longer than anything a browser address bar is comfortable with. A link past
/// this is not a link someone meant to click.
const MAX_LENGTH: usize = 4096;

/// The link to open, in the form it will be opened in, or why it will not be.
pub fn vet(link: &str) -> Result<String, String> {
    if link.len() > MAX_LENGTH {
        return Err("that link is too long to open".into());
    }
    if link.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("that link has spaces or control characters in it".into());
    }
    let parsed = Url::parse(link).map_err(|_| "that is not a link afterm can read".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(format!(
            "afterm opens only web links, and this one is {}:",
            parsed.scheme()
        ));
    }
    if parsed.host_str().is_none_or(str::is_empty) {
        return Err("that link names no host".into());
    }
    // The parser's own spelling: percent-encoded and normalised, so what the
    // shell receives is exactly what was checked.
    Ok(parsed.into())
}

/// Hand a vetted link to the default browser.
#[cfg(windows)]
pub fn open(link: &str) -> Result<(), String> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let target = HSTRING::from(vet(link)?);
    let verb = HSTRING::from("open");
    // SAFETY: every pointer is to an HSTRING that lives until the call returns,
    // and the null ones are documented as optional.
    let result = unsafe {
        ShellExecuteW(None, &verb, &target, PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL)
    };
    // Anything above 32 is success; the rest are the legacy error codes.
    if result.0 as isize > 32 {
        Ok(())
    } else {
        Err(format!("Windows would not open the link (error {})", result.0 as isize))
    }
}

/// Hand a vetted link to the default browser. The development stand-in:
/// `xdg-open` with the link as one argument, never through a shell.
#[cfg(not(windows))]
pub fn open(link: &str) -> Result<(), String> {
    let target = vet(link)?;
    std::process::Command::new("xdg-open")
        .arg(target)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("could not start a browser: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_web_link_is_opened_as_the_parser_spells_it() {
        assert_eq!(vet("https://example.com/a b").unwrap_err(), "that link has spaces or control characters in it");
        assert_eq!(vet("https://example.com/docs").unwrap(), "https://example.com/docs");
        assert_eq!(vet("HTTP://Example.COM").unwrap(), "http://example.com/");
        assert_eq!(vet("https://example.com/caf%C3%A9?q=1#x").unwrap(), "https://example.com/caf%C3%A9?q=1#x");
    }

    #[test]
    fn nothing_but_the_web_is_passed_to_the_operating_system() {
        for link in [
            "file:///C:/Windows/System32/calc.exe",
            "search-ms:query=secret&crumb=location:\\\\evil\\share",
            "ms-msdt:/id PCWDiagnostic",
            "javascript:alert(1)",
            "ssh://host",
            "mailto:a@example.com",
        ] {
            assert!(vet(link).is_err(), "{link} should be refused");
        }
    }

    #[test]
    fn a_link_with_nowhere_to_go_is_refused() {
        assert!(vet("https://").is_err());
        assert!(vet("not a link").is_err());
        assert!(vet(&format!("https://example.com/{}", "a".repeat(MAX_LENGTH))).is_err());
        assert!(vet("https://example.com/\u{7}").is_err());
    }
}
