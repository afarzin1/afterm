//! An image on the Windows clipboard, as a PNG file a remote program can open.
//!
//! A coding agent in an SSH session reads images from the clipboard of the
//! machine it runs on, which is the host, and the screenshot is on this one.
//! So `Ctrl Shift V` with an image and no text sends the image to the host and
//! types its path instead — which is what the agent accepts in place of a
//! paste, and what a shell can use as well.
//!
//! arboard can read images, but only by decoding them into pixels, and its
//! image support is the `image` crate with its colour-management crates behind
//! it — the dependency the manifest leaves out on purpose. It is not needed:
//! Greenshot, the Snipping Tool and browsers all put a finished PNG on the
//! clipboard under the registered `PNG` format, and those bytes go to the host
//! as they are. Only a source that offers nothing but a bitmap — the Print
//! Screen key, some older programs — needs converting, and a device-independent
//! bitmap is simple enough to convert here, with `png` doing the encoding.

// The converter is reached only from the Windows clipboard, and from the tests
// everywhere; a development build elsewhere would otherwise call it dead.
#![cfg_attr(not(windows), allow(dead_code))]

use crate::fsmodel::{self, EntryKind};
use crate::localfs;
use crate::transport::{self, Collision, Failure, Transport};

/// How long a pasted image is kept on the host. Long enough for the agent
/// session it was pasted into, short enough that screenshots do not pile up in
/// a folder nobody looks in.
pub const KEEP_DAYS: u64 = 7;

/// The most pixels a bitmap may claim before it is refused rather than
/// allocated for. Eight 8K screens; a clipboard bitmap larger than that is
/// corrupt, not a screenshot.
const MAX_PIXELS: u64 = 8 * 7680 * 4320;

#[cfg(windows)]
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// The clipboard's image as PNG bytes, or `None` when it holds no image.
#[cfg(windows)]
pub fn clipboard_png() -> Result<Option<Vec<u8>>, Failure> {
    use clipboard_win::{formats::CF_DIB, is_format_avail, raw::get_vec, register_format};

    // Another program can hold the clipboard open for a moment; arboard gives
    // it the same handful of retries.
    let _open = clipboard_win::Clipboard::new_attempts(10)
        .map_err(|error| Failure::operation(format!("could not open the clipboard: {error}")))?;

    if let Some(format) = register_format("PNG").map(u32::from).filter(|&id| is_format_avail(id)) {
        let mut data = Vec::new();
        if get_vec(format, &mut data).is_ok() && data.starts_with(PNG_SIGNATURE) {
            return Ok(Some(data));
        }
    }
    // Windows offers `CF_DIB` for anything that put a bitmap of any kind on
    // the clipboard, converting it itself where it has to.
    if is_format_avail(CF_DIB) {
        let mut data = Vec::new();
        get_vec(CF_DIB, &mut data)
            .map_err(|error| Failure::operation(format!("could not read the clipboard image: {error}")))?;
        return dib_to_png(&data).map(Some).map_err(Failure::operation);
    }
    Ok(None)
}

/// Development builds elsewhere have no Windows clipboard to read.
#[cfg(not(windows))]
pub fn clipboard_png() -> Result<Option<Vec<u8>>, Failure> {
    Ok(None)
}

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

/// One colour channel out of a packed pixel, widened or narrowed to 8 bits.
fn channel(pixel: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let value = (pixel & mask) >> mask.trailing_zeros();
    let top = mask >> mask.trailing_zeros();
    (u64::from(value) * 255 / u64::from(top)) as u8
}

/// Convert a clipboard `CF_DIB` — a `BITMAPINFOHEADER` or one of its longer
/// versions, then the pixels — to a PNG.
///
/// 24- and 32-bit only, which is every screen capture: a palette is what a
/// bitmap from 1995 has. A 32-bit pixel's fourth byte is used as alpha only
/// when the header names an alpha mask *and* some pixel uses it, because
/// `BI_RGB` leaves that byte undefined and most programs leave it zero — taken
/// at its word, every such screenshot would arrive fully transparent.
pub fn dib_to_png(dib: &[u8]) -> Result<Vec<u8>, String> {
    const BI_RGB: u32 = 0;
    const BI_BITFIELDS: u32 = 3;
    const BI_ALPHABITFIELDS: u32 = 6;
    let short = || "the clipboard image is cut short".to_string();

    let header = u32_at(dib, 0).ok_or_else(short)? as usize;
    if header < 40 {
        return Err(format!("the clipboard image has a header afterm does not know ({header} bytes)"));
    }
    let width = i32::from_le_bytes(dib.get(4..8).ok_or_else(short)?.try_into().unwrap());
    let height = i32::from_le_bytes(dib.get(8..12).ok_or_else(short)?.try_into().unwrap());
    let depth = u16_at(dib, 14).ok_or_else(short)?;
    let compression = u32_at(dib, 16).ok_or_else(short)?;
    let palette = u32_at(dib, 32).ok_or_else(short)? as usize;

    if width <= 0 || height == 0 {
        return Err("the clipboard image has no pixels".into());
    }
    let (width, bottom_up) = (width as usize, height > 0);
    let height = height.unsigned_abs() as usize;
    if (width as u64) * (height as u64) > MAX_PIXELS {
        return Err(format!("the clipboard image is implausibly large ({width} × {height})"));
    }
    if depth != 24 && depth != 32 {
        return Err(format!("afterm cannot convert a {depth}-bit clipboard image; save it as a file instead"));
    }

    // The masks sit inside a V4 or V5 header, and straight after a plain one.
    let (masks, after_masks) = match compression {
        BI_RGB => ([0x00ff_0000, 0x0000_ff00, 0x0000_00ff, 0], header),
        BI_BITFIELDS | BI_ALPHABITFIELDS => {
            let count = if header >= 56 { 4 } else if compression == BI_ALPHABITFIELDS { 4 } else { 3 };
            let at = if header >= 52 { 40 } else { header };
            let mut masks = [0u32; 4];
            for (index, mask) in masks.iter_mut().enumerate().take(count) {
                *mask = u32_at(dib, at + index * 4).ok_or_else(short)?;
            }
            (masks, if header >= 52 { header } else { header + count * 4 })
        }
        other => return Err(format!("afterm cannot convert a compressed clipboard image (type {other})")),
    };
    // A V4/V5 header's alpha mask is there whatever the compression.
    let alpha_mask = if header >= 56 { u32_at(dib, 52).ok_or_else(short)? } else { masks[3] };
    if depth == 24 && compression != BI_RGB {
        return Err("the clipboard image pairs 24-bit pixels with colour masks".into());
    }

    let start = after_masks + palette * 4;
    let stride = (width * depth as usize).div_ceil(32) * 4;
    let pixels = dib.get(start..start + stride * height).ok_or_else(short)?;
    let row = |y: usize| {
        let source = if bottom_up { height - 1 - y } else { y };
        &pixels[source * stride..source * stride + stride]
    };

    let alpha = depth == 32
        && alpha_mask != 0
        && (0..height).any(|y| row(y)[..width * 4].chunks_exact(4).any(|px| {
            u32::from_le_bytes(px.try_into().unwrap()) & alpha_mask != 0
        }));
    let channels = if alpha { 4 } else { 3 };
    let mut out = Vec::with_capacity(width * height * channels);
    for y in 0..height {
        let line = row(y);
        for x in 0..width {
            if depth == 24 {
                let px = &line[x * 3..x * 3 + 3];
                out.extend_from_slice(&[px[2], px[1], px[0]]);
            } else {
                let px = u32::from_le_bytes(line[x * 4..x * 4 + 4].try_into().unwrap());
                out.extend_from_slice(&[channel(px, masks[0]), channel(px, masks[1]), channel(px, masks[2])]);
                if alpha {
                    out.push(channel(px, alpha_mask));
                }
            }
        }
    }

    let mut png = Vec::new();
    let mut encoder = png::Encoder::new(&mut png, width as u32, height as u32);
    encoder.set_color(if alpha { png::ColorType::Rgba } else { png::ColorType::Rgb });
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|error| error.to_string())?;
    writer.write_image_data(&out).map_err(|error| error.to_string())?;
    writer.finish().map_err(|error| error.to_string())?;
    Ok(png)
}

/// Year, month and day of a count of days since 1970-01-01, proleptic
/// Gregorian. Howard Hinnant's `civil_from_days`.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

fn date(seconds: u64) -> String {
    let (year, month, day) = civil((seconds / 86_400) as i64);
    format!("{year:04}{month:02}{day:02}")
}

/// The name a pasted image goes to the host under: when it was pasted, in
/// UTC, and four random hex digits so two pastes in a second do not collide.
///
/// No spaces and nothing a shell would expand, because the path is typed into
/// a terminal as it is. Which is also why a taken name is never answered with
/// the usual `name (1)`.
pub fn paste_name(seconds: u64, tag: u16) -> String {
    let time = seconds % 86_400;
    format!(
        "paste-{}-{:02}{:02}{:02}-{tag:04x}.png",
        date(seconds),
        time / 3600,
        time / 60 % 60,
        time % 60
    )
}

/// Whether `name` is a pasted image older than `KEEP_DAYS`.
///
/// Decided from the name rather than the host's modification time, which the
/// two transports report in different shapes, and only for names that
/// `paste_name` could have made: anything else in the folder is someone's, and
/// left alone.
pub fn is_stale(name: &str, now: u64) -> bool {
    let Some(rest) = name.strip_prefix("paste-").and_then(|rest| rest.strip_suffix(".png")) else {
        return false;
    };
    let parts: Vec<&str> = rest.split('-').collect();
    let shaped = matches!(parts.as_slice(), [day, time, tag]
        if day.len() == 8 && day.bytes().all(|b| b.is_ascii_digit())
        && time.len() == 6 && time.bytes().all(|b| b.is_ascii_digit())
        && tag.len() == 4 && tag.bytes().all(|b| b.is_ascii_hexdigit()));
    shaped && parts[0] < date(now.saturating_sub(KEEP_DAYS * 86_400)).as_str()
}

/// Where on the host a pasted image goes, under its home directory. Made
/// private when afterm creates it: a screenshot is as likely as anything to
/// have something on it that the other accounts on a shared host should not
/// see.
const FOLDER: [&str; 3] = [".cache", "afterm", "paste"];

/// Put `png` in the paste folder under `home` on the host, making the folder
/// if it is not there, and return where it landed and the folder it is in.
///
/// `home` is the account's home directory, which the caller asks the transport
/// for; a test names a directory of its own rather than write into a real one.
pub async fn send(
    transport: &Transport,
    home: &str,
    png: Vec<u8>,
    now: u64,
) -> Result<(String, String), Failure> {
    let directory = FOLDER.iter().fold(home.to_string(), |path, part| fsmodel::join(&path, part));
    if !transport.exists(&directory).await? {
        let mut path = home.to_string();
        for part in FOLDER {
            path = fsmodel::join(&path, part);
            // Checked rather than attempted: a `mkdir` refused because the
            // folder is there looks the same as one refused for any other
            // reason.
            if !transport.exists(&path).await? {
                transport.create_directory(&path).await?;
            }
        }
        transport.make_private(&directory).await?;
    }

    let name = paste_name(now, rand::random::<u16>());
    let scratch = std::env::temp_dir();
    let staged = format!("afterm-paste-{:016x}.png", rand::random::<u64>());
    let part = transport::blocking(move || {
        let part = localfs::claim(&scratch, &staged)?;
        std::fs::write(&part, &png).map(|()| part.clone()).map_err(|error| {
            localfs::discard(&part);
            Failure::internal(format!("could not stage the image: {error}"))
        })
    })
    .await?;
    let local = part.to_string_lossy().to_string();
    let sent = transport.upload(&local, &directory, &name, Collision::Fail, |_, _| {}).await;
    localfs::discard(&part);
    Ok((sent?, directory))
}

/// Delete the pasted images in `directory` older than `KEEP_DAYS`. Best effort
/// and silent: nothing is waiting on it, and a file left over is harmless.
pub async fn tidy(transport: &Transport, directory: &str, now: u64) {
    let Ok(listing) = transport.list(directory).await else { return };
    for entry in listing.entries {
        if entry.kind == EntryKind::File && is_stale(&entry.name, now) {
            let _ = transport.delete(&entry.path, false).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `CF_DIB` as Windows lays one out: header, masks where the
    /// compression wants them, then rows padded to four bytes.
    fn dib(width: i32, height: i32, depth: u16, compression: u32, masks: &[u32], rows: &[Vec<u8>]) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&40u32.to_le_bytes());
        data.extend_from_slice(&width.to_le_bytes());
        data.extend_from_slice(&height.to_le_bytes());
        data.extend_from_slice(&1u16.to_le_bytes());
        data.extend_from_slice(&depth.to_le_bytes());
        data.extend_from_slice(&compression.to_le_bytes());
        data.extend_from_slice(&[0; 20]);
        for mask in masks {
            data.extend_from_slice(&mask.to_le_bytes());
        }
        for row in rows {
            data.extend_from_slice(row);
        }
        data
    }

    fn decode(png: &[u8]) -> (png::ColorType, u32, u32, Vec<u8>) {
        let mut reader = png::Decoder::new(std::io::Cursor::new(png)).read_info().unwrap();
        let mut buffer = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buffer).unwrap();
        buffer.truncate(info.buffer_size());
        (info.color_type, info.width, info.height, buffer)
    }

    #[test]
    fn a_bottom_up_24_bit_bitmap_comes_out_the_right_way_up_in_rgb() {
        // Two by two: the file's first row is the picture's bottom row. Each
        // row is six bytes of BGR padded to eight.
        let bottom = vec![0, 0, 255, 0, 255, 0, 0, 0]; // red, green
        let top = vec![255, 0, 0, 255, 255, 255, 0, 0]; // blue, white
        let png = dib_to_png(&dib(2, 2, 24, 0, &[], &[bottom, top])).unwrap();
        let (color, width, height, pixels) = decode(&png);
        assert_eq!((color, width, height), (png::ColorType::Rgb, 2, 2));
        assert_eq!(pixels, [0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 255, 0]);
    }

    #[test]
    fn a_zero_fourth_byte_is_not_read_as_transparent() {
        let rows = vec![vec![10, 20, 30, 0]];
        let png = dib_to_png(&dib(1, -1, 32, 0, &[], &rows)).unwrap();
        let (color, _, _, pixels) = decode(&png);
        assert_eq!(color, png::ColorType::Rgb);
        assert_eq!(pixels, [30, 20, 10]);
    }

    #[test]
    fn an_alpha_mask_that_is_used_keeps_its_alpha() {
        let masks = [0x00ff_0000, 0x0000_ff00, 0x0000_00ff, 0xff00_0000];
        let rows = vec![vec![10, 20, 30, 128, 1, 2, 3, 255]];
        let png = dib_to_png(&dib(2, -1, 32, 6, &masks, &rows)).unwrap();
        let (color, _, _, pixels) = decode(&png);
        assert_eq!(color, png::ColorType::Rgba);
        assert_eq!(pixels, [30, 20, 10, 128, 3, 2, 1, 255]);
    }

    #[test]
    fn a_truncated_or_palette_bitmap_is_refused_rather_than_guessed_at() {
        let full = dib(2, 2, 24, 0, &[], &[vec![0; 8], vec![0; 8]]);
        assert!(dib_to_png(&full[..full.len() - 1]).is_err());
        assert!(dib_to_png(&dib(2, 2, 8, 0, &[], &[vec![0; 4], vec![0; 4]])).is_err());
        assert!(dib_to_png(&dib(0, 2, 24, 0, &[], &[])).is_err());
    }

    #[test]
    fn names_carry_the_utc_time_and_nothing_a_shell_would_touch() {
        // 2026-09-25 14:30:12 UTC.
        assert_eq!(paste_name(1_790_346_612, 0x3f9a), "paste-20260925-143012-3f9a.png");
        assert_eq!(paste_name(0, 0), "paste-19700101-000000-0000.png");
        // A leap day, and the day after.
        assert_eq!(&paste_name(951_782_400, 1)[6..14], "20000229");
        assert_eq!(&paste_name(951_868_800, 1)[6..14], "20000301");
    }

    #[test]
    fn only_old_pastes_are_stale_and_nothing_else_in_the_folder_ever_is() {
        let now = 1_790_346_612; // 2026-09-25
        assert!(is_stale("paste-20260917-235959-0000.png", now));
        assert!(!is_stale("paste-20260918-000000-0000.png", now));
        assert!(!is_stale(&paste_name(now, 7), now));
        assert!(!is_stale("paste-20200101-000000-0000.png.keep", now));
        assert!(!is_stale("notes-20200101-000000-0000.png", now));
        assert!(!is_stale("paste-2020010-000000-0000.png", now));
        assert!(!is_stale("paste-20200101-000000-00000.png", now));
    }
}
