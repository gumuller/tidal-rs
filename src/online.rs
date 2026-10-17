use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::{
    env,
    error::Error,
    fs::{self, DirBuilder, OpenOptions},
    io::{BufReader, Write},
    os::unix::{
        fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
        net::UnixStream,
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tidalrs::{AudioQuality, Authz, TidalClient, Track};
use tokio::io::AsyncBufReadExt;

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const PAGE_SIZE: u32 = 50;
// Public device-client defaults used by python-tidal and TidalCLI. These are
// application identifiers, not a user's account credentials. See THIRD_PARTY.md.
const DEVICE_CLIENT: &str = "fX2JxdmntZWK0ixT";
const DEVICE_SECRET: &str = "1Nn9AfDAjxrgJFJbKNWLeAyKGVGmINuXPPLHVXAvxAg=";

pub(crate) fn state_directory() -> Result<PathBuf> {
    let root = match env::var_os("XDG_STATE_HOME") {
        Some(path) if Path::new(&path).is_absolute() => PathBuf::from(path),
        _ => PathBuf::from(env::var_os("HOME").ok_or("HOME is unavailable")?).join(".local/state"),
    };
    let dir = root.join("tidal-rs");
    DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    Ok(dir)
}

pub(crate) fn save_auth(path: &Path, auth: &Authz) -> Result<()> {
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp)?;
    let result = (|| -> Result<()> {
        file.write_all(&serde_json::to_vec(auth)?)?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

pub(crate) fn client(token_path: &Path) -> Result<TidalClient> {
    let token_path = token_path.to_owned();
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    Ok(
        TidalClient::new(env::var("TIDAL_CLIENT_ID").unwrap_or_else(|_| DEVICE_CLIENT.into()))
            .with_client_secret(
                env::var("TIDAL_CLIENT_SECRET").unwrap_or_else(|_| DEVICE_SECRET.into()),
            )
            .with_client(http)
            .with_max_backoff_millis(5000)
            .with_authz_refresh_callback(move |auth| {
                if let Err(error) = save_auth(&token_path, &auth) {
                    eprintln!("Could not save refreshed session: {error}");
                }
            }),
    )
}

async fn login(token_path: &Path) -> Result<TidalClient> {
    login_with(token_path, |url, code| {
        println!("\nSign in on TIDAL’s website: {url}\nDevice code: {code}");
        println!("Waiting for your browser authorization…");
    })
    .await
}

pub(crate) async fn login_with(
    token_path: &Path,
    show_code: impl FnOnce(&str, &str),
) -> Result<TidalClient> {
    let client = client(token_path)?;
    let flow = client.device_authorization().await?;
    let url = if flow.url.starts_with("https://") {
        flow.url.clone()
    } else {
        format!("https://{}", flow.url)
    };
    if !url.starts_with("https://link.tidal.com/") {
        return Err("Unexpected TIDAL authorization URL".into());
    }
    show_code(&url, &flow.user_code);
    let _ = Command::new("xdg-open")
        .arg(&url)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let token = tokio::time::timeout(
        Duration::from_secs(flow.expires_in),
        client.wait_for_authorization(&flow.device_code, flow.expires_in, flow.interval),
    )
    .await??;
    save_auth(token_path, &token.authz())?;
    println!("Signed in successfully.\n");
    Ok(client)
}

pub(crate) struct Player {
    child: Child,
    dir: PathBuf,
    writer: UnixStream,
    reader: BufReader<UnixStream>,
    id: u64,
}

impl Player {
    pub(crate) fn start(silent: bool) -> Result<Self> {
        let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let dir = env::temp_dir().join(format!("tidal-rs-online-{}-{unique}", std::process::id()));
        DirBuilder::new().mode(0o700).create(&dir)?;
        let socket = dir.join("mpv.sock");
        let mut process = Command::new("mpv");
        if silent && std::env::var_os("TIDAL_TEST_SINK").is_none() {
            process.arg("--ao=null");
        } else {
            process.arg("--ao=pulse").arg(format!(
                "--audio-client-name=tidal-rs-{}",
                std::process::id()
            ));
            if let Ok(sink) = std::env::var("TIDAL_TEST_SINK") {
                process.arg(format!("--audio-device=pulse/{sink}"));
            }
        }
        let mut child = process
            .args([
                "--no-config",
                "--idle=yes",
                "--no-video",
                "--no-terminal",
                "--volume=35",
                "--keep-open=yes",
                "--demuxer-lavf-o=protocol_whitelist=[file,https,tcp,tls,crypto,data]",
            ])
            .arg(format!("--input-ipc-server={}", socket.display()))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(writer) = UnixStream::connect(&socket) {
                writer.set_read_timeout(Some(Duration::from_secs(10)))?;
                let reader = BufReader::new(writer.try_clone()?);
                return Ok(Self {
                    child,
                    dir,
                    writer,
                    reader,
                    id: 0,
                });
            }
            if child.try_wait()?.is_some() || Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                let _ = fs::remove_dir_all(&dir);
                return Err("mpv failed to start its audio backend".into());
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    pub(crate) fn command(&mut self, args: Value) -> Result<Value> {
        self.id += 1;
        writeln!(
            self.writer,
            "{}",
            json!({"command":args, "request_id":self.id})
        )?;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            self.writer.set_read_timeout(Some(
                deadline
                    .checked_duration_since(Instant::now())
                    .ok_or("mpv timed out")?,
            ))?;
            let line = tidal_rs::read_frame(&mut self.reader)?.ok_or("mpv disconnected")?;
            let reply: Value = serde_json::from_str(&line)?;
            if reply["request_id"].as_u64() == Some(self.id) {
                if reply["error"] != "success" {
                    return Err(format!("mpv: {}", reply["error"]).into());
                }
                return Ok(reply["data"].clone());
            }
        }
    }

    pub(crate) fn property(&mut self, name: &str) -> Result<Value> {
        self.command(json!(["get_property", name]))
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.dir);
    }
}

enum Browse {
    Tracks,
    Playlists(Vec<(String, String)>),
    Albums(Vec<(u64, String)>),
}
pub(crate) struct Library {
    pub(crate) tracks: Vec<Track>,
    view: Browse,
    pub(crate) current: Option<usize>,
    pub(crate) quality: AudioQuality,
    pub(crate) actual_quality: Option<AudioQuality>,
}

impl Library {
    pub(crate) fn new() -> Self {
        Self {
            tracks: vec![],
            view: Browse::Tracks,
            current: None,
            quality: AudioQuality::High,
            actual_quality: None,
        }
    }

    fn show_tracks(&self, total: usize, page: u32) {
        for (i, track) in self.tracks.iter().enumerate() {
            let artist = track
                .artists
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            println!("{:>3}. {} — {}", i + 1, track.title, artist);
        }
        println!(
            "\n{} tracks shown, {total} total, page {page}. Use play NUMBER.",
            self.tracks.len()
        );
    }

    async fn browse(&mut self, client: &TidalClient, kind: &str, page: u32) -> Result<()> {
        let offset = page
            .saturating_sub(1)
            .checked_mul(PAGE_SIZE)
            .ok_or("Page too large")?;
        match kind {
            "tracks" => {
                let list = client
                    .favorite_tracks(Some(offset), Some(PAGE_SIZE), None, None)
                    .await?;
                self.tracks = list.items.into_iter().map(|f| f.item).collect();
                self.view = Browse::Tracks;
                self.show_tracks(list.total, page);
            }
            "playlists" => {
                let list = client.user_playlists(Some(offset), Some(PAGE_SIZE)).await?;
                let rows: Vec<_> = list.items.into_iter().map(|p| (p.uuid, p.title)).collect();
                for (i, (_, title)) in rows.iter().enumerate() {
                    println!("{:>3}. {title}", i + 1);
                }
                println!(
                    "\n{} shown, {} total, page {page}. Use open NUMBER.",
                    rows.len(),
                    list.total
                );
                self.view = Browse::Playlists(rows);
            }
            "albums" => {
                let list = client
                    .favorite_albums(Some(offset), Some(PAGE_SIZE), None, None)
                    .await?;
                let rows: Vec<_> = list
                    .items
                    .into_iter()
                    .map(|a| (a.item.id, a.item.title))
                    .collect();
                for (i, (_, title)) in rows.iter().enumerate() {
                    println!("{:>3}. {title}", i + 1);
                }
                println!(
                    "\n{} shown, {} total, page {page}. Use open NUMBER.",
                    rows.len(),
                    list.total
                );
                self.view = Browse::Albums(rows);
            }
            _ => return Err("Use tracks, playlists, or albums".into()),
        }
        Ok(())
    }

    async fn open(&mut self, client: &TidalClient, number: usize, page: u32) -> Result<()> {
        let index = number
            .checked_sub(1)
            .ok_or("Choose a number starting at 1")?;
        let offset = page
            .saturating_sub(1)
            .checked_mul(PAGE_SIZE)
            .ok_or("Page too large")?;
        let list = match &self.view {
            Browse::Playlists(rows) => {
                let (id, title) = rows.get(index).ok_or("Playlist number out of range")?;
                println!("\n{title}");
                client
                    .playlist_tracks(id, Some(offset), Some(PAGE_SIZE))
                    .await?
            }
            Browse::Albums(rows) => {
                let (id, title) = rows.get(index).ok_or("Album number out of range")?;
                println!("\n{title}");
                client
                    .album_tracks(*id, Some(offset), Some(PAGE_SIZE))
                    .await?
            }
            Browse::Tracks => return Err("List playlists or albums first, then open NUMBER".into()),
        };
        self.tracks = list.items;
        self.show_tracks(list.total, page);
        Ok(())
    }

    pub(crate) async fn play(
        &mut self,
        client: &TidalClient,
        player: &mut Player,
        index: usize,
    ) -> Result<()> {
        let track = self
            .tracks
            .get(index)
            .ok_or("Track number out of range; use tracks first")?;
        println!("Loading {}…", track.title);
        let mut info = client
            .track_dash_playback_info(track.id, self.quality)
            .await?;
        // Some v1 sessions return AAC for LOSSLESS but provide FLAC via the
        // Hi-Res negotiation path. Never silently accept that downgrade.
        if self.quality == AudioQuality::Lossless
            && matches!(info.audio_quality, AudioQuality::Low | AudioQuality::High)
        {
            if let Ok(lossless) = client
                .track_dash_playback_info(track.id, AudioQuality::HiResLossless)
                .await
            {
                if matches!(
                    lossless.audio_quality,
                    AudioQuality::Lossless | AudioQuality::HiResLossless
                ) {
                    info = lossless;
                }
            }
        }
        let source = if info.manifest_mime_type.split(';').next()
            == Some("application/vnd.tidal.bts")
        {
            let manifest: tidalrs::BtsManifest =
                serde_json::from_slice(&STANDARD.decode(&info.manifest)?)?;
            if manifest.encryption_type != "NONE" {
                return Err("This stream requires an unsupported encrypted playback path".into());
            }
            let url = manifest
                .urls
                .first()
                .ok_or("TIDAL returned no stream URL")?;
            if !url.starts_with("https://") {
                return Err("TIDAL returned an unexpected media URL".into());
            }
            url.to_owned()
        } else if info.manifest_mime_type.split(';').next() == Some("application/dash+xml") {
            let bytes = STANDARD.decode(&info.manifest)?;
            let xml = std::str::from_utf8(&bytes)?;
            if xml.contains("ContentProtection") {
                return Err("This DASH stream requires a DRM backend".into());
            }
            let path = player.dir.join(format!("stream-{}.mpd", track.id));
            let mut file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&path)?;
            file.write_all(&bytes)?;
            path.to_string_lossy().into_owned()
        } else {
            return Err(format!("Unsupported stream manifest: {}", info.manifest_mime_type).into());
        };
        player.command(json!(["stop"]))?;
        player.command(json!(["loadfile", source, "replace"]))?;
        player.command(json!(["set_property", "pause", false]))?;
        // Confirm loading rather than declaring success at socket delivery.
        let started = Instant::now();
        let deadline = started + Duration::from_secs(20);
        loop {
            if player
                .property("duration")
                .ok()
                .and_then(|v| v.as_f64())
                .is_some_and(|d| d > 0.0)
            {
                println!("Playing: {} ({:?})", track.title, info.audio_quality);
                self.current = Some(index);
                self.actual_quality = Some(info.audio_quality);
                return Ok(());
            }

            if started.elapsed() > Duration::from_secs(1)
                && player
                    .property("idle-active")
                    .ok()
                    .and_then(|v| v.as_bool())
                    == Some(true)
            {
                return Err("The audio stream could not be decoded by mpv".into());
            }
            if Instant::now() >= deadline {
                return Err("Stream loading timed out".into());
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    }
}

fn help() {
    println!(
        "\ntracks [PAGE]      Favorite tracks\nplaylists [PAGE]   Your playlists\nalbums [PAGE]      Saved albums\nopen N [PAGE]     Show tracks in playlist/album N\nplay N            Play track N; continue through this page\npause | resume | stop | next\nvolume 0..100 | seek SECONDS | status\nquality high|lossless  Choose requested quality (availability varies)\nlogin | logout | help | quit\n"
    );
}

fn prompt() {
    print!("tidal> ");
    let _ = std::io::stdout().flush();
}

pub async fn run() -> Result<()> {
    println!(
        "TIDAL Rust — online library\nSign in through TIDAL’s website; your password stays there."
    );
    let directory = state_directory()?;
    let token_path = directory.join("session.json");
    let mut client = match fs::read(&token_path) {
        Ok(bytes) => client(&token_path)?.with_authz(serde_json::from_slice(&bytes)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => login(&token_path).await?,
        Err(e) => return Err(e.into()),
    };
    let mut library = Library::new();
    let mut player = Player::start(false)?;
    help();
    if let Err(error) = library.browse(&client, "tracks", 1).await {
        println!(
            "Could not load favorite tracks: {error}\nUse login to authorize again, or playlists to browse playlists."
        );
    }
    let mut input = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    prompt();
    loop {
        tokio::select! {
            line = input.next_line() => {
                let Some(line) = line? else {break};
                let args:Vec<_> = line.split_whitespace().collect();
                let verb = args.first().copied().unwrap_or("");
                if matches!(verb,"q"|"quit"|"exit") {break;}
                let outcome: Result<()> = async {
                    let number = || -> Result<usize> {Ok(args.get(1).ok_or("Enter a number")?.parse()?)};
                    let page = |slot:usize| -> Result<u32> { let p=args.get(slot).map(|s|s.parse()).transpose()?.unwrap_or(1); if p==0 {Err("Page must start at 1".into())} else {Ok(p)} };
                    match verb {
                        "" => {},
                        "help" => help(),
                        "tracks"|"playlists"|"albums" => { library.current=None; library.browse(&client,verb,page(1)?).await?; },
                        "open" => {library.current=None; library.open(&client,number()?,page(2)?).await?;},
                        "play" => {let index=number()?.checked_sub(1).ok_or("Track numbers start at 1")?; library.current=None; library.play(&client,&mut player,index).await?;},
                        "next" => {let index=library.current.ok_or("No active queue")?+1; library.current=None; library.play(&client,&mut player,index).await?;},
                        "pause" => {player.command(json!(["set_property","pause",true]))?;},
                        "resume" => {player.command(json!(["set_property","pause",false]))?;},
                        "stop" => {library.current=None;player.command(json!(["stop"]))?;},
                        "seek" => {let time:f64=args.get(1).ok_or("Enter seconds")?.parse()?; if !time.is_finite()||time<0.0{return Err("Invalid seek time".into())};player.command(json!(["seek",time,"absolute+exact"]))?;},
                        "volume" => {let volume=number()?;if volume>100{return Err("Volume must be 0–100".into())};player.command(json!(["set_property","volume",volume]))?;},
                        "status" => {println!("Time: {} s; paused: {}; volume: {}",player.property("time-pos").unwrap_or(Value::Null),player.property("pause")?,player.property("volume")?);},
                        "quality" => {library.quality=match args.get(1).copied(){Some("high")=>AudioQuality::High,Some("lossless")=>AudioQuality::Lossless,_=>return Err("Use quality high or quality lossless".into())};println!("Requested quality updated for the next track.");},
                        "login" => {library.current=None;player.command(json!(["stop"]))?;client=login(&token_path).await?;library=Library::new();library.browse(&client,"tracks",1).await?;},
                        "logout" => {player.command(json!(["stop"]))?;if token_path.exists(){fs::remove_file(&token_path)?;}println!("Saved session removed. Closing player.");return Ok(());},
                        _ => println!("Unknown command. Type help."),
                    }
                    Ok(())
                }.await;
                if verb=="logout" && outcome.is_ok() { break; }
                if let Err(error)=outcome {println!("Error: {error}");}
                prompt();
            },
            _ = tick.tick() => {
                if let Some(index)=library.current {
                    if player.property("eof-reached").ok().and_then(|v|v.as_bool())==Some(true) {
                        library.current=None;
                        if index+1<library.tracks.len() {
                            println!();
                            if let Err(error)=library.play(&client,&mut player,index+1).await {println!("Playback stopped: {error}");}
                        } else {println!("\nEnd of the current page.");}
                        prompt();
                    }
                }
            }
        }
    }
    Ok(())
}

/// Live read-only library check plus a silent stream-decoding check.
pub async fn check() -> Result<()> {
    let token_path = state_directory()?.join("session.json");
    let auth: Authz = serde_json::from_slice(&fs::read(&token_path)?)?;
    let client = client(&token_path)?.with_authz(auth);
    let tracks = client.favorite_tracks(Some(0), Some(1), None, None).await?;
    println!("Favorite tracks available: {}", tracks.total);
    let playlists = client.user_playlists(Some(0), Some(1)).await?;
    println!("User playlists available: {}", playlists.total);
    let albums = client.favorite_albums(Some(0), Some(1), None, None).await?;
    println!("Saved albums available: {}", albums.total);
    let mut library = Library::new();
    library.tracks = tracks.items.into_iter().map(|f| f.item).collect();
    if library.tracks.is_empty() {
        if let Some(playlist) = playlists.items.first() {
            library.tracks = client
                .playlist_tracks(&playlist.uuid, Some(0), Some(1))
                .await?
                .items;
        } else if let Some(album) = albums.items.first() {
            library.tracks = client
                .album_tracks(album.item.id, Some(0), Some(1))
                .await?
                .items;
        }
    }
    if library.tracks.is_empty() {
        println!("No track found for a stream check.");
        return Ok(());
    }
    let mut player = Player::start(true)?;
    library.play(&client, &mut player, 0).await?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let position = player.property("time-pos")?.as_f64().unwrap_or(0.0);
    if position <= 0.0 {
        return Err("Stream loaded but its playback clock did not advance".into());
    }
    println!("PASS: authenticated stream loaded, decoded, and advanced (silent output).");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn session_storage_is_private_and_roundtrips() {
        let dir = env::temp_dir().join(format!("tidal-rs-session-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.json");
        let auth = Authz::new(
            "test-access".into(),
            Some("test-refresh".into()),
            Some(1),
            Some("IE".into()),
            Some(999),
        );
        save_auth(&path, &auth).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let restored: Authz = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(restored.access_token, "test-access");
        save_auth(&path, &auth).unwrap();
        fs::remove_dir_all(dir).unwrap();
    }
}
