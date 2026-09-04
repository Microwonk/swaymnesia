//! Binary encoding of a session file.
//!
//! All integers are little-endian. A `str` is a u32 byte length followed by that many bytes of
//! UTF-8.
//!
//!     header     magic [4]u8 = "SWMN"
//!                version u16 = 1
//!                workspace count u32
//!     workspace  name str
//!                output str
//!                layout u8, one of 0 splith, 1 splitv, 2 stacked, 3 tabbed
//!                window count u32
//!     window     app_id str, empty when the view reports none
//!                title str
//!                argv count u32, followed by that many str
//!                flags u8, bit 0 floating, bit 1 fullscreen, bit 2 focused
//!                rect i32 x, y, width, height, only meaningful when floating
//!     trailer    scratchpad window count u32
//!                window, repeated that many times
//!
//! Workspaces and windows are stored in the order sway reports them, which is the order they are
//! recreated in.

use crate::session::{Layout, Rect, Session, Window, Workspace};
use std::fmt;

const MAGIC: [u8; 4] = *b"SWMN";
const VERSION: u16 = 1;

const FLAG_FLOATING: u8 = 1 << 0;
const FLAG_FULLSCREEN: u8 = 1 << 1;
const FLAG_FOCUSED: u8 = 1 << 2;

#[derive(Debug)]
pub enum Error {
    BadMagic,
    UnsupportedVersion(u16),
    Truncated,
    BadLayout(u8),
    BadUtf8,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::BadMagic => write!(f, "not a swaymnesia session file"),
            Error::UnsupportedVersion(v) => write!(f, "unsupported session format version {v}"),
            Error::Truncated => write!(f, "session file truncated"),
            Error::BadLayout(v) => write!(f, "unknown layout {v}"),
            Error::BadUtf8 => write!(f, "session file contains invalid UTF-8"),
        }
    }
}

impl std::error::Error for Error {}

pub fn encode(session: &Session) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    put_u32(&mut out, session.workspaces.len());

    for workspace in &session.workspaces {
        put_str(&mut out, &workspace.name);
        put_str(&mut out, &workspace.output);
        out.push(workspace.layout as u8);
        put_windows(&mut out, &workspace.windows);
    }

    put_windows(&mut out, &session.scratchpad);
    out
}

fn put_windows(out: &mut Vec<u8>, windows: &[Window]) {
    put_u32(out, windows.len());
    for window in windows {
        put_str(out, &window.app_id);
        put_str(out, &window.title);
        put_u32(out, window.argv.len());

        for arg in &window.argv {
            put_str(out, arg);
        }

        let mut flags = 0;
        if window.floating {
            flags |= FLAG_FLOATING;
        }
        if window.fullscreen {
            flags |= FLAG_FULLSCREEN;
        }
        if window.focused {
            flags |= FLAG_FOCUSED;
        }
        out.push(flags);

        for value in [
            window.rect.x,
            window.rect.y,
            window.rect.width,
            window.rect.height,
        ] {
            out.extend_from_slice(&value.to_le_bytes());
        }
    }
}

pub fn decode(bytes: &[u8]) -> Result<Session, Error> {
    let mut r = Reader { bytes, pos: 0 };
    if r.take(4)? != MAGIC {
        return Err(Error::BadMagic);
    }
    let version = r.u16()?;
    if version != VERSION {
        return Err(Error::UnsupportedVersion(version));
    }

    // do not allocate with workspace length (r.u32()) in case the file is corrupt.
    // also wouldn't gain much as noone's going to have *that* many workspaces
    let mut workspaces = Vec::new();
    for _ in 0..r.u32()? {
        let name = r.str()?;
        let output = r.str()?;
        let raw_layout = r.u8()?;
        let layout = Layout::from_u8(raw_layout).ok_or(Error::BadLayout(raw_layout))?;

        let windows = take_windows(&mut r)?;
        workspaces.push(Workspace {
            name,
            output,
            layout,
            windows,
        });
    }
    let scratchpad = take_windows(&mut r)?;

    Ok(Session {
        workspaces,
        scratchpad,
    })
}

fn take_windows(r: &mut Reader<'_>) -> Result<Vec<Window>, Error> {
    // same as in decode above regarding premature allocation
    let mut windows = Vec::new();
    for _ in 0..r.u32()? {
        let app_id = r.str()?;
        let title = r.str()?;
        let mut argv = Vec::new();
        for _ in 0..r.u32()? {
            argv.push(r.str()?);
        }
        let flags = r.u8()?;
        let rect = Rect {
            x: r.i32()?,
            y: r.i32()?,
            width: r.i32()?,
            height: r.i32()?,
        };
        windows.push(Window {
            app_id,
            title,
            argv,
            floating: flags & FLAG_FLOATING != 0,
            fullscreen: flags & FLAG_FULLSCREEN != 0,
            focused: flags & FLAG_FOCUSED != 0,
            rect,
        });
    }
    Ok(windows)
}

fn put_u32(out: &mut Vec<u8>, value: usize) {
    out.extend_from_slice(&(value as u32).to_le_bytes());
}

fn put_str(out: &mut Vec<u8>, value: &str) {
    put_u32(out, value.len());
    out.extend_from_slice(value.as_bytes());
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], Error> {
        let end = self.pos.checked_add(len).ok_or(Error::Truncated)?;
        let slice = self.bytes.get(self.pos..end).ok_or(Error::Truncated)?;
        self.pos = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, Error> {
        Ok(u16::from_le_bytes(
            self.take(2)?.try_into().expect("2 bytes"),
        ))
    }

    fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        ))
    }

    fn i32(&mut self) -> Result<i32, Error> {
        Ok(i32::from_le_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        ))
    }

    fn str(&mut self) -> Result<String, Error> {
        let len = self.u32()? as usize;
        let bytes = self.take(len)?.to_vec();
        String::from_utf8(bytes).map_err(|_| Error::BadUtf8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Session {
        Session {
            workspaces: vec![
                Workspace {
                    name: "1".to_string(),
                    output: "eDP-1".to_string(),
                    layout: Layout::Tabbed,
                    windows: vec![
                        Window {
                            app_id: "foot".to_string(),
                            title: "zsh".to_string(),
                            argv: vec!["foot".to_string(), "-e".to_string(), "top".to_string()],
                            floating: false,
                            fullscreen: true,
                            focused: true,
                            rect: Rect {
                                x: 0,
                                y: 0,
                                width: 1920,
                                height: 1080,
                            },
                        },
                        Window {
                            app_id: String::new(),
                            title: "it's a \u{e9}dge case; rm -rf".to_string(),
                            argv: vec!["sh".to_string(), "-c".to_string(), "echo 'hi'".to_string()],
                            floating: true,
                            fullscreen: false,
                            focused: false,
                            rect: Rect {
                                x: -20,
                                y: 40,
                                width: 800,
                                height: 600,
                            },
                        },
                    ],
                },
                Workspace {
                    name: "web".to_string(),
                    output: "HDMI-A-1".to_string(),
                    layout: Layout::SplitV,
                    windows: Vec::new(),
                },
            ],
            scratchpad: vec![Window {
                app_id: "signal".to_string(),
                title: "Signal".to_string(),
                argv: vec!["signal-desktop".to_string()],
                floating: true,
                fullscreen: false,
                focused: false,
                rect: Rect {
                    x: 639,
                    y: 207,
                    width: 1309,
                    height: 1077,
                },
            }],
        }
    }

    #[test]
    fn round_trips() {
        assert_eq!(decode(&encode(&sample())).unwrap(), sample());
    }

    #[test]
    fn round_trips_empty_session() {
        let empty = Session {
            workspaces: Vec::new(),
            scratchpad: Vec::new(),
        };
        assert_eq!(decode(&encode(&empty)).unwrap(), empty);
    }

    #[test]
    fn keeps_scratchpad_apart_from_workspaces() {
        let decoded = decode(&encode(&sample())).unwrap();
        assert_eq!(decoded.scratchpad, sample().scratchpad);
        assert!(
            decoded
                .workspaces
                .iter()
                .all(|w| w.windows.iter().all(|v| v.app_id != "signal"))
        );
    }

    #[test]
    fn rejects_foreign_data() {
        assert!(matches!(decode(b"nope"), Err(Error::BadMagic)));
    }

    #[test]
    fn rejects_unknown_version() {
        let mut bytes = encode(&sample());
        bytes[4..6].copy_from_slice(&99u16.to_le_bytes());
        assert!(matches!(decode(&bytes), Err(Error::UnsupportedVersion(99))));
    }

    #[test]
    fn rejects_truncated() {
        let bytes = encode(&sample());
        assert!(matches!(
            decode(&bytes[..bytes.len() - 4]),
            Err(Error::Truncated)
        ));
    }

    #[test]
    fn rejects_oversized_count() {
        let mut bytes = encode(&sample());
        bytes[6..10].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(decode(&bytes), Err(Error::Truncated)));
    }
}
