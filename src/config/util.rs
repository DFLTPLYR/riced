//! Path expansion and image decoding shared by config and shell surfaces.
use super::BackgroundImage;
use std::path::PathBuf;

impl BackgroundImage {
    /// Files arrive as `file://` URIs (pickers, drag-drop) or plain paths
    /// (hand-written config). The image pipeline wants a plain path.
    ///
    /// Accepted forms, in resolution order: `file://` scheme stripped,
    /// `$VAR`/`${VAR}` expanded from the environment, then a leading `~`
    /// expanded to the home dir. Prefer `~/Pictures/…` in hand-written
    /// config: unlike `$CUSTOM_VAR` it doesn't depend on the daemon's
    /// environment, and unlike absolute paths it survives username changes.
    pub fn local_path(&self) -> PathBuf {
        let s = self.path.trim();
        let stripped = s.strip_prefix("file://").unwrap_or(s);
        let stripped = stripped.strip_prefix("localhost").unwrap_or(stripped);
        let expanded = expand_env(&percent_decode(stripped));
        expand_tilde(&expanded)
    }
}

/// Expand `$VAR` and `${VAR}` from the environment. Undefined vars and lone
/// `$` pass through untouched (the path then simply fails to open downstream).
fn expand_env(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        let braced = chars.peek() == Some(&'{');
        if braced {
            chars.next();
        }
        let mut name = String::new();
        while let Some(&ch) = chars.peek() {
            let take = if braced {
                ch != '}'
            } else {
                ch.is_alphanumeric() || ch == '_'
            };
            if take {
                name.push(ch);
                chars.next();
            } else {
                break;
            }
        }
        if braced {
            if chars.peek() == Some(&'}') {
                chars.next();
            } else {
                // Unterminated `${…`: emit literally.
                out.push_str("${");
                out.push_str(&name);
                continue;
            }
        }
        if name.is_empty() {
            out.push('$');
            continue;
        }
        match std::env::var(&name) {
            Ok(v) => out.push_str(&v),
            Err(_) => {
                out.push('$');
                if braced {
                    out.push('{');
                    out.push_str(&name);
                    out.push('}');
                } else {
                    out.push_str(&name);
                }
            }
        }
    }
    out
}

/// Expand a leading `~` / `~/` to the home dir (`dirs` crate, already used
/// for the config dir). Anything else passes through; `~otheruser` is
/// intentionally unsupported.
fn expand_tilde(s: &str) -> PathBuf {
    if s == "~"
        && let Some(home) = dirs::home_dir()
    {
        return home;
    } else if let Some(rest) = s.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(s)
}

/// Minimal `%XX` decoder for file URIs (`%20` spaces etc.). Malformed
/// sequences pass through untouched.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (
                hex_val(bytes.get(i + 1).copied().unwrap_or(0)),
                hex_val(bytes.get(i + 2).copied().unwrap_or(0)),
            )
        {
            out.push(h << 4 | l);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Synchronously decode a wallpaper file to RGBA bytes for a pre-warmed
/// [`Handle`](iced::widget::image::Handle). File-backed handles decode on a
/// worker thread whose completion redraw the shell drops, leaving first paint
/// blank — serving `from_rgba` instead loads synchronously ("very cheap" per
/// the renderer) so pixels exist on the very first frame. `None` for missing
/// or undecodable files (caller skips the entry, same as before).
pub(crate) fn decode_handle(
    path: &std::path::Path,
) -> Option<(u32, u32, iced::widget::image::Handle)> {
    use iced::widget::image::Handle;

    let img = image::open(path).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    if w == 0 || h == 0 {
        return None;
    }
    let handle = Handle::from_rgba(w, h, bytes::Bytes::from(rgba.into_raw()));
    Some((w, h, handle))
}
#[cfg(test)]
mod tests {
    use super::super::BackgroundImage;
    use super::*;

    #[test]
    fn decode_handle_roundtrips_a_real_file() {
        use image::{ImageBuffer, Rgba};

        let dir = std::env::temp_dir().join(format!("riced-decode-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("tiny.png");
        ImageBuffer::<Rgba<u8>, _>::from_pixel(3, 2, Rgba([9, 8, 7, 255]))
            .save(&path)
            .unwrap();

        let (w, h, handle) = decode_handle(&path).expect("decodable png");
        assert_eq!((w, h), (3, 2));
        // Handle carries the pre-decoded pixels (Rgba variant, sync load).
        assert!(matches!(handle, iced::widget::image::Handle::Rgba { .. }));

        assert!(decode_handle(&dir.join("missing.png")).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn local_path_handles_uris_and_plain_paths() {
        let plain = BackgroundImage {
            path: "/home/u/pic.png".into(),
            ..Default::default()
        };
        assert_eq!(plain.local_path(), PathBuf::from("/home/u/pic.png"));

        let uri = BackgroundImage {
            path: "file:///home/dfltplyr/Pictures/Wallpaper/handcamera.png".into(),
            ..Default::default()
        };
        assert_eq!(
            uri.local_path(),
            PathBuf::from("/home/dfltplyr/Pictures/Wallpaper/handcamera.png")
        );

        let host = BackgroundImage {
            path: "file://localhost/home/u/my%20pic.png".into(),
            ..Default::default()
        };
        assert_eq!(host.local_path(), PathBuf::from("/home/u/my pic.png"));

        let empty = BackgroundImage::default();
        assert!(empty.local_path().as_os_str().is_empty());
    }

    #[test]
    fn local_path_expands_tilde_and_env() {
        if let Some(home) = dirs::home_dir() {
            let tilde = BackgroundImage {
                path: "~/Pictures/handcamera.png".into(),
                ..Default::default()
            };
            assert_eq!(tilde.local_path(), home.join("Pictures/handcamera.png"));
        }

        // Deterministic vars (unique names: tests run in parallel).
        // SAFETY: test-only, unique var name, no other thread touches it.
        unsafe { std::env::set_var("RICED_TEST_PICS", "/pics") };
        let dollar = BackgroundImage {
            path: "$RICED_TEST_PICS/w.png".into(),
            ..Default::default()
        };
        assert_eq!(dollar.local_path(), PathBuf::from("/pics/w.png"));
        let braced = BackgroundImage {
            path: "${RICED_TEST_PICS}/w.png".into(),
            ..Default::default()
        };
        assert_eq!(braced.local_path(), PathBuf::from("/pics/w.png"));
        let combo = BackgroundImage {
            path: "file://$RICED_TEST_PICS/my%20pic.png".into(),
            ..Default::default()
        };
        assert_eq!(combo.local_path(), PathBuf::from("/pics/my pic.png"));

        // Undefined vars and lone `$` pass through untouched.
        let undef = BackgroundImage {
            path: "$RICED_TEST_UNDEFINED_XYZ/w.png".into(),
            ..Default::default()
        };
        assert_eq!(
            undef.local_path(),
            PathBuf::from("$RICED_TEST_UNDEFINED_XYZ/w.png")
        );
    }
}
