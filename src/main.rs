use std::{
    env,
    error::Error,
    io::{self, BufReader, Write},
    os::unix::net::UnixStream,
    time::Duration,
};
use tidal_rs::{Command, parse_signal, read_frame};
mod control;
mod gui;
mod online;
mod theme;

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = env::args().skip(1).collect();
    let mode = args.first().map(String::as_str).unwrap_or("help");
    if mode == "--version" {
        println!("tidal-rs {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if mode == "control" {
        return control::run(&args[1..]);
    }
    if mode == "gui" {
        gui::run();
        return Ok(());
    }
    if mode == "online" {
        return tokio::runtime::Runtime::new()?.block_on(online::run());
    }
    if mode == "online-check" {
        return tokio::runtime::Runtime::new()?.block_on(online::check());
    }
    if mode == "help" || mode == "--help" {
        println!(
            "tidal-rs — native Linux music client\n\n\
            gui                Open the native GTK4 music player\n\
            control COMMAND    Control playback or show/hide the running player\n\
            translate          Read observed command JSONL; emit proposed mpv JSONL\n\
            signals            Read player signal JSONL; normalize optional prefixes\n\
            send SOCKET        Translate commands and send to a running mpv IPC socket\n\n\
            online             Log in, browse your TIDAL library, and stream music\n\n\
            See research/FINDINGS.md for evidence and remaining work."
        );
        return Ok(());
    }
    if !matches!(mode, "translate" | "signals" | "send") {
        return Err(format!("unknown command: {mode}; use --help").into());
    }
    let mut socket = if mode == "send" {
        Some(UnixStream::connect(
            args.get(1).ok_or("send requires an mpv socket path")?,
        )?)
    } else {
        None
    };
    let mut replies = match socket.as_ref() {
        Some(socket) => {
            socket.set_read_timeout(Some(Duration::from_secs(10)))?;
            Some(BufReader::new(socket.try_clone()?))
        }
        None => None,
    };
    let mut request_id = 0_u64;
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    while let Some(line) = read_frame(&mut input)? {
        if line.trim().is_empty() {
            continue;
        }
        let mut value = if mode == "signals" {
            serde_json::to_value(parse_signal(&line)?)?
        } else {
            serde_json::from_str::<Command>(&line)?.to_mpv()?
        };
        if let Some(socket) = socket.as_mut() {
            request_id += 1;
            value["request_id"] = request_id.into();
            let encoded = serde_json::to_string(&value)?;
            writeln!(socket, "{encoded}")?;
            let replies = replies.as_mut().ok_or("missing mpv reply stream")?;
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            loop {
                let remaining = deadline
                    .checked_duration_since(std::time::Instant::now())
                    .ok_or("mpv acknowledgement timed out")?;
                socket.set_read_timeout(Some(remaining))?;
                let reply =
                    read_frame(replies)?.ok_or("mpv disconnected before acknowledgement")?;
                let reply: serde_json::Value = serde_json::from_str(&reply)?;
                if reply["request_id"].as_u64() == Some(request_id) {
                    if reply["error"] != "success" {
                        return Err(format!("mpv rejected command: {}", reply["error"]).into());
                    }
                    break;
                }
            }
        } else {
            let encoded = serde_json::to_string(&value)?;
            writeln!(output, "{encoded}")?;
        }
    }
    Ok(())
}
