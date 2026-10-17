use serde_json::json;
use std::io::{BufReader, Cursor};
use tidal_rs::{Command, MAX_FRAME_BYTES, parse_signal, read_frame};

#[test]
fn fragmented_crlf_and_multiple_frames() {
    let mut input = BufReader::with_capacity(
        3,
        Cursor::new(
            b"{\"signal\":\"version\"}\r\n{\"signal\":\"media.state\",\"state\":\"playing\"}\n",
        ),
    );
    assert_eq!(
        parse_signal(&read_frame(&mut input).unwrap().unwrap())
            .unwrap()
            .signal,
        "version"
    );
    assert_eq!(
        parse_signal(&read_frame(&mut input).unwrap().unwrap())
            .unwrap()
            .fields["state"],
        "playing"
    );
    assert!(read_frame(&mut input).unwrap().is_none());
}

#[test]
fn prefix_and_json_colons_preserve_unknown_fields() {
    let signal =
        parse_signal("player:{\"signal\":\"future.signal\",\"url\":\"https://example.com/a\"}")
            .unwrap();
    assert_eq!(signal.fields["url"], "https://example.com/a");
    assert_eq!(
        parse_signal("{\"signal\":\"version\"}").unwrap().signal,
        "version"
    );
}

#[test]
fn command_schema_and_adapter_validation() {
    let load: Command = serde_json::from_value(json!({"command":"media.load", "url":"file:///tmp/tone.wav", "streamFormat":"flac", "encryptionKey":""})).unwrap();
    assert_eq!(
        load.to_mpv().unwrap(),
        json!({"command":["loadfile","file:///tmp/tone.wav","replace"]})
    );
    assert_eq!(
        Command::Volume { volume: 0.5 }.to_mpv().unwrap()["command"][2],
        50.0
    );
    assert!(Command::Volume { volume: 2.0 }.to_mpv().is_err());
    assert!(Command::Seek { time: -1.0 }.to_mpv().is_err());
    assert!(serde_json::from_str::<Command>(r#"{"command":"media.play","typo":1}"#).is_err());
    let encrypted: Command = serde_json::from_value(json!({"command":"media.load", "url":"https://example.com/a", "streamFormat":"flac", "encryptionKey":"test"})).unwrap();
    assert!(encrypted.to_mpv().is_err());
}

#[test]
fn rejects_unterminated_oversized_and_invalid_utf8_frames() {
    assert!(read_frame(&mut Cursor::new(b"{}")).is_err());
    assert!(read_frame(&mut Cursor::new(vec![b'x'; MAX_FRAME_BYTES + 1])).is_err());
    assert!(read_frame(&mut Cursor::new(b"\xff\n")).is_err());
}
