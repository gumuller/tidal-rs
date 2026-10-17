//! Independently implemented protocol subset observed in TIDAL Desktop 2.43.2.
//! This is an offline workbench, not a streaming service or authenticated client.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::{self, BufRead};

pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

#[derive(Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "command", deny_unknown_fields)]
pub enum Command {
    #[serde(rename = "media.load")]
    Load {
        url: String,
        #[serde(rename = "streamFormat")]
        stream_format: String,
        #[serde(rename = "encryptionKey", default)]
        encryption_key: String,
    },
    #[serde(rename = "media.play")]
    Play {},
    #[serde(rename = "media.pause")]
    Pause {},
    #[serde(rename = "media.stop")]
    Stop {},
    #[serde(rename = "media.seek")]
    Seek { time: f64 },
    #[serde(rename = "media.volume")]
    Volume { volume: f64 },
}

impl Command {
    /// Proposed mpv adapter; units and load/pause policy are design choices,
    /// not a claim of native TIDAL player compatibility.
    pub fn to_mpv(&self) -> Result<Value, &'static str> {
        let command = match self {
            Self::Load {
                url,
                encryption_key,
                ..
            } => {
                if !encryption_key.is_empty() {
                    return Err("encrypted playback is not implemented");
                }
                if !(url.starts_with("https://") || url.starts_with("file:///")) {
                    return Err("expected an https:// or absolute file:/// media URL");
                }
                json!(["loadfile", url, "replace"])
            }
            Self::Play {} => json!(["set_property", "pause", false]),
            Self::Pause {} => json!(["set_property", "pause", true]),
            Self::Stop {} => json!(["stop"]),
            Self::Seek { time } if time.is_finite() && *time >= 0.0 => {
                json!(["seek", time, "absolute+exact"])
            }
            Self::Seek { .. } => return Err("seek time must be finite and nonnegative"),
            Self::Volume { volume } if volume.is_finite() && (0.0..=1.0).contains(volume) => {
                json!(["set_property", "volume", volume * 100.0])
            }
            Self::Volume { .. } => return Err("adapter volume must be between 0 and 1"),
        };
        Ok(json!({"command": command}))
    }
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
pub struct Signal {
    pub signal: String,
    #[serde(flatten)]
    pub fields: serde_json::Map<String, Value>,
}

/// Accept bare JSON or a single opaque prefix followed by ':' and JSON.
/// The wrapper strips the prefix; this parser also accepts bare JSON safely.
pub fn parse_signal(line: &str) -> serde_json::Result<Signal> {
    let line = line.trim();
    let payload = if line.starts_with('{') {
        line
    } else {
        line.split_once(':')
            .map_or(line, |(_, payload)| payload.trim())
    };
    serde_json::from_str(payload)
}

/// A bounded line reader handles arbitrary pipe fragmentation and CRLF/LF.
/// Unlike the shipped JS chunk splitting, partial frames remain buffered.
pub fn read_frame(reader: &mut impl BufRead) -> io::Result<Option<String>> {
    let mut frame = Vec::new();
    loop {
        let chunk = reader.fill_buf()?;
        if chunk.is_empty() {
            return if frame.is_empty() {
                Ok(None)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "unterminated JSON frame",
                ))
            };
        }
        let newline = chunk.iter().position(|b| *b == b'\n');
        let count = newline.map_or(chunk.len(), |n| n + 1);
        if frame.len() + count > MAX_FRAME_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "JSON frame exceeds limit",
            ));
        }
        frame.extend_from_slice(&chunk[..count]);
        reader.consume(count);
        if newline.is_some() {
            frame.pop();
            if frame.last() == Some(&b'\r') {
                frame.pop();
            }
            return String::from_utf8(frame)
                .map(Some)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e));
        }
    }
}
