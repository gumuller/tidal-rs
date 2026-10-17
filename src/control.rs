//! Session D-Bus controls. Only deliberately public playback metadata is exposed.
use gtk::{gio, glib, prelude::*};
use std::{
    collections::HashMap,
    error::Error,
    process::{Command, Stdio},
};

pub fn run(args: &[String]) -> Result<(), Box<dyn Error>> {
    let command = args.first().map(String::as_str).unwrap_or("status");
    let (action, parameter) = match command {
        "status" | "show" | "hide" | "toggle-window" | "toggle-playback" | "next" | "previous" | "quit" => {
            if args.len() > 1 { return Err("This control takes no argument".into()); }
            (command, None)
        }
        "volume" | "seek" => {
            if args.len() != 2 { return Err("Expected one numeric argument".into()); }
            let value: f64 = args[1].parse()?;
            if !value.is_finite() || value < 0.0 || (command == "volume" && value > 100.0) {
                return Err("Volume must be 0–100; seek must be nonnegative seconds".into());
            }
            (command, Some(value.to_variant()))
        }
        _ => return Err("Unknown control; use status, show, hide, toggle-window, toggle-playback, next, previous, volume, seek, or quit".into()),
    };
    let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)?;
    let id = std::env::var("TIDAL_APP_ID").unwrap_or_else(|_| "io.github.tidalrs.Player".into());
    let path = format!("/{}", id.replace('.', "/").replace('-', "_"));
    let owner = bus.call_sync(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "NameHasOwner",
        Some(&(id.as_str(),).to_variant()),
        None,
        gio::DBusCallFlags::NONE,
        3000,
        gio::Cancellable::NONE,
    )?;
    if !owner.child_value(0).get::<bool>().unwrap_or(false) {
        if command == "status" {
            println!("{{\"running\":false,\"visible\":false}}");
            return Ok(());
        }
        if matches!(command, "show" | "toggle-window") {
            Command::new(std::env::current_exe()?)
                .arg("gui")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?;
            return Ok(());
        }
        return Err("TIDAL Rust is not running; use tidal-rs control show".into());
    }
    if command == "status" {
        let result = bus.call_sync(
            Some(&id),
            &path,
            "org.gtk.Actions",
            "Describe",
            Some(&("status",).to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            3000,
            gio::Cancellable::NONE,
        )?;
        let state = result.child_value(0).child_value(2);
        let json = state
            .child_value(0)
            .as_variant()
            .and_then(|v| v.get::<String>())
            .ok_or("Player does not support status; upgrade tidal-rs")?;
        println!("{json}");
    } else {
        let parameters: Vec<glib::Variant> = parameter.into_iter().collect();
        let platform: HashMap<String, glib::Variant> = HashMap::new();
        bus.call_sync(
            Some(&id),
            &path,
            "org.gtk.Actions",
            "Activate",
            Some(&(action, parameters, platform).to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            3000,
            gio::Cancellable::NONE,
        )?;
    }
    Ok(())
}
