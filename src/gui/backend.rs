use super::storage::{History, Preferences, Quality};
use crate::online::{self, Library, Player};
use serde_json::json;
use std::{
    collections::HashSet,
    fs,
    path::PathBuf,
    sync::{Arc, mpsc},
    time::Duration,
};
use tidalrs::{AudioQuality, Authz, TidalClient, Track};
use tokio::sync::{Semaphore, mpsc as channel};

pub const PAGE_SIZE: u32 = 48;
#[derive(Clone, Debug)]
pub enum View {
    Home,
    Explore,
    Discovery {
        path: String,
        title: String,
        section: Option<String>,
    },
    SearchAlbums(String),
    Favorites,
    Albums,
    Playlists,
    Album {
        id: u64,
        title: String,
    },
    Playlist {
        id: String,
        title: String,
    },
    Search(String),
}
impl View {
    pub fn title(&self) -> String {
        match self {
            Self::Home => "Home".into(),
            Self::Explore => "Explore".into(),
            Self::Discovery { title, section, .. } => section
                .as_ref()
                .map(|s| format!("{title} · {s}"))
                .unwrap_or_else(|| title.clone()),
            Self::SearchAlbums(q) => format!("Albums for “{q}”"),
            Self::Favorites => "Favorite tracks".into(),
            Self::Albums => "Your albums".into(),
            Self::Playlists => "Your playlists".into(),
            Self::Album { title, .. } | Self::Playlist { title, .. } => title.clone(),
            Self::Search(q) => format!("Results for “{q}”"),
        }
    }
    pub fn is_grid(&self) -> bool {
        if let Self::Discovery {
            section: Some(section),
            ..
        } = self
        {
            return !section.to_lowercase().contains("track");
        }
        matches!(
            self,
            Self::Home
                | Self::Albums
                | Self::Playlists
                | Self::Explore
                | Self::Discovery { section: None, .. }
                | Self::SearchAlbums(_)
        )
    }
}
#[derive(Clone, Debug)]
pub enum Key {
    Discovery(String),
    Track(usize),
    Album(u64),
    Playlist(String),
}
#[derive(Clone, Debug)]
pub struct Item {
    pub key: Key,
    pub title: String,
    pub subtitle: String,
    pub cover: Option<String>,
    pub detail: String,
    pub album: String,
}
pub struct Page {
    pub epoch: u64,
    pub view: View,
    pub page: u32,
    pub total: usize,
    pub sections: Vec<String>,
    pub groups: Vec<(String, Vec<Item>)>,
    pub items: Vec<Item>,
}
#[derive(Clone)]
pub struct Playback {
    pub title: String,
    pub album_title: String,
    pub album_id: u64,
    pub requested: String,
    pub codec: String,
    pub artist: String,
    pub cover: Option<String>,
    pub position: f64,
    pub duration: f64,
    pub paused: bool,
    pub quality: String,
    pub can_next: bool,
    pub can_previous: bool,
}
pub enum Action {
    Browse {
        view: View,
        page: u32,
        epoch: u64,
    },
    Play {
        index: usize,
        epoch: u64,
    },
    Suggest {
        query: String,
        generation: u64,
        albums: bool,
    },
    PlayQueued(usize),
    FavoriteAlbum {
        id: u64,
        saved: bool,
    },
    Toggle,
    Next,
    Previous,
    Seek(f64),
    Volume(f64),
    Quality(u32),
    Login,
    Logout,
    Shutdown,
}
pub enum Event {
    Suggestions {
        query: String,
        generation: u64,
        items: Vec<Item>,
    },
    Queue(Vec<Item>),
    SavedAlbums(Vec<u64>),
    AlbumSaved(u64, bool),
    Ready,
    NeedsLogin,
    LoginCode(String, String),
    Page(Page),
    Artwork(String, PathBuf),
    Playback(Playback),
    LoadingTrack(String),
    Error(String),
}

fn artists(track: &Track) -> String {
    track
        .artists
        .iter()
        .map(|a| a.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}
fn track_items(tracks: &[Track]) -> Vec<Item> {
    tracks
        .iter()
        .enumerate()
        .map(|(index, t)| Item {
            key: Key::Track(index),
            title: t.title.clone(),
            subtitle: artists(t),
            cover: t.album.cover.clone(),
            detail: format!("{}:{:02}", t.duration / 60, t.duration % 60),
            album: t.album.title.clone(),
        })
        .collect()
}

async fn fetch(
    client: &TidalClient,
    view: &View,
    page: u32,
    epoch: u64,
) -> Result<(Page, Vec<Track>), Box<dyn std::error::Error>> {
    let offset = page.saturating_sub(1).saturating_mul(PAGE_SIZE);
    let mut sections = vec![];
    let mut groups = vec![];
    let (items, total, tracks) = match view {
        View::Home => {
            let (sections, tracks) = home(client).await?;
            groups = sections;
            let items: Vec<_> = groups
                .iter()
                .flat_map(|(_, items)| items.iter().cloned())
                .collect();
            (items.clone(), items.len(), tracks)
        }
        View::Explore | View::Discovery { .. } => {
            client.favorite_tracks(Some(0), Some(1), None, None).await?;
            let path = match view {
                View::Discovery { path, .. } => path.as_str(),
                _ => "pages/explore",
            };
            let mut value = discovery_page(client, path, 0).await?;
            sections = modules(&value)
                .filter(|m| {
                    matches!(
                        m["type"].as_str(),
                        Some("ALBUM_LIST" | "TRACK_LIST" | "PLAYLIST_LIST")
                    )
                })
                .filter_map(|m| {
                    m["title"]
                        .as_str()
                        .filter(|t| !t.is_empty())
                        .map(str::to_owned)
                })
                .collect();
            let selected = match view {
                View::Discovery {
                    section: Some(section),
                    ..
                } => Some(section.as_str()),
                _ => None,
            };
            if let Some(section) = selected {
                let module = modules(&value)
                    .find(|m| m["title"].as_str() == Some(section))
                    .cloned();
                if let Some(mut module) = module {
                    if let Some(path) = module["pagedList"]["dataApiPath"].as_str() {
                        let list = discovery_page(client, path, offset).await?;
                        module["pagedList"] = list;
                    }
                    value = json!({"rows":[{"modules":[module]}]});
                } else {
                    value = json!({"rows":[]});
                }
            }
            let is_tracks = selected.is_some_and(|s| s.to_lowercase().contains("track"));
            if is_tracks {
                let mut tracks = vec![];
                let mut total = 0;
                for module in modules(&value).filter(|m| m["type"] == "TRACK_LIST") {
                    total += module["pagedList"]["totalNumberOfItems"]
                        .as_u64()
                        .unwrap_or(0) as usize;
                    if let Some(items) = module["pagedList"]["items"].as_array() {
                        for item in items {
                            tracks.push(serde_json::from_value::<Track>(item.clone())?);
                        }
                    }
                }
                (track_items(&tracks), total, tracks)
            } else {
                let all = discovery_items(&value);
                let remote = if selected.is_some() {
                    modules(&value)
                        .filter_map(|m| m["pagedList"]["totalNumberOfItems"].as_u64())
                        .max()
                        .unwrap_or(all.len() as u64) as usize
                } else {
                    all.len()
                };
                let already_paged = selected.is_some()
                    && modules(&value).any(|m| m["pagedList"]["offset"].as_u64().unwrap_or(0) > 0);
                let total = remote.max(all.len());
                (
                    all.into_iter()
                        .skip(if already_paged { 0 } else { offset as usize })
                        .take(PAGE_SIZE as usize)
                        .collect(),
                    total,
                    vec![],
                )
            }
        }

        View::SearchAlbums(query) => {
            let albums: tidalrs::List<tidalrs::Album> =
                catalog_search(client, "albums", query, offset).await?;
            let items = albums
                .items
                .into_iter()
                .map(|a| Item {
                    key: Key::Album(a.id),
                    title: a.title,
                    subtitle: a
                        .artists
                        .iter()
                        .map(|a| a.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    cover: a.cover,
                    detail: format!("{} tracks", a.number_of_tracks),
                    album: String::new(),
                })
                .collect();
            (items, albums.total, vec![])
        }
        View::Favorites => {
            let list = client
                .favorite_tracks(Some(offset), Some(PAGE_SIZE), None, None)
                .await?;
            let tracks: Vec<_> = list.items.into_iter().map(|t| t.item).collect();
            (track_items(&tracks), list.total, tracks)
        }
        View::Albums => {
            let list = client
                .favorite_albums(Some(offset), Some(PAGE_SIZE), None, None)
                .await?;
            let items = list
                .items
                .into_iter()
                .map(|a| Item {
                    key: Key::Album(a.item.id),
                    title: a.item.title,
                    subtitle: a
                        .item
                        .artists
                        .iter()
                        .map(|a| a.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    cover: a.item.cover,
                    detail: format!("{} tracks", a.item.number_of_tracks),
                    album: String::new(),
                })
                .collect();
            (items, list.total, vec![])
        }
        View::Playlists => {
            let list = client.user_playlists(Some(offset), Some(PAGE_SIZE)).await?;
            let items = list
                .items
                .into_iter()
                .map(|p| Item {
                    key: Key::Playlist(p.uuid),
                    title: p.title,
                    subtitle: format!("{} tracks", p.number_of_tracks),
                    cover: p.image,
                    detail: "Playlist".into(),
                    album: String::new(),
                })
                .collect();
            (items, list.total, vec![])
        }
        View::Album { id, .. } => {
            let list = client.album_tracks(*id, Some(0), Some(100)).await?;
            let total = list.total;
            let mut tracks = list.items;
            while tracks.len() < total {
                let more = client
                    .album_tracks(*id, Some(tracks.len() as u32), Some(100))
                    .await?;
                if more.items.is_empty() {
                    return Err("Incomplete album track list".into());
                }
                tracks.extend(more.items);
            }
            (track_items(&tracks), total, tracks)
        }
        View::Playlist { id, .. } => {
            let list = client
                .playlist_tracks(id, Some(offset), Some(PAGE_SIZE))
                .await?;
            (track_items(&list.items), list.total, list.items)
        }
        View::Search(query) => {
            let tracks: tidalrs::List<Track> =
                catalog_search(client, "tracks", query, offset).await?;
            (track_items(&tracks.items), tracks.total, tracks.items)
        }
    };
    Ok((
        Page {
            epoch,
            view: view.clone(),
            page,
            total,
            sections,
            groups,
            items,
        },
        tracks,
    ))
}

fn modules(value: &serde_json::Value) -> impl Iterator<Item = &serde_json::Value> {
    value["rows"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|row| row["modules"].as_array().into_iter().flatten())
}
async fn discovery_page(
    client: &TidalClient,
    path: &str,
    offset: u32,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    if !path.starts_with("pages/") || path.contains("..") || path.contains(['?', '#', ':']) {
        return Err("Invalid discovery path".into());
    }
    let auth = client.get_authz().ok_or("Please sign in")?;
    let body = reqwest::Client::builder()
        .timeout(Duration::from_secs(25))
        .build()?
        .get(format!("https://api.tidal.com/v1/{path}"))
        .bearer_auth(&auth.access_token)
        .query(&[
            ("countryCode", client.get_country_code()),
            ("deviceType", "BROWSER".into()),
            ("locale", "en_US".into()),
            ("offset", offset.to_string()),
            ("limit", PAGE_SIZE.to_string()),
        ])
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    Ok(serde_json::from_str(&body)?)
}

async fn catalog_search<T: serde::de::DeserializeOwned>(
    client: &TidalClient,
    kind: &str,
    query: &str,
    offset: u32,
) -> Result<tidalrs::List<T>, Box<dyn std::error::Error>> {
    client.favorite_tracks(Some(0), Some(1), None, None).await?;
    let auth = client.get_authz().ok_or("Please sign in")?;
    let body = reqwest::Client::builder()
        .timeout(Duration::from_secs(25))
        .build()?
        .get(format!("https://api.tidal.com/v1/search/{kind}"))
        .bearer_auth(&auth.access_token)
        .query(&[
            ("query", query.to_owned()),
            ("countryCode", client.get_country_code()),
            ("offset", offset.to_string()),
            ("limit", PAGE_SIZE.to_string()),
            ("includeDidYouMean", "false".into()),
        ])
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    Ok(serde_json::from_str(&body)?)
}

async fn suggestions(
    auth: &Authz,
    country: &str,
    query: &str,
    albums: bool,
) -> Result<Vec<Item>, Box<dyn std::error::Error>> {
    let kind = if albums { "albums" } else { "tracks" };
    let body = reqwest::Client::builder()
        .timeout(Duration::from_secs(6))
        .build()?
        .get(format!("https://api.tidal.com/v1/search/{kind}"))
        .bearer_auth(&auth.access_token)
        .query(&[("countryCode", country), ("query", query), ("limit", "6")])
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    if albums {
        let list: tidalrs::List<tidalrs::Album> = serde_json::from_str(&body)?;
        Ok(list
            .items
            .into_iter()
            .map(|a| Item {
                key: Key::Album(a.id),
                title: a.title,
                subtitle: a
                    .artists
                    .iter()
                    .map(|a| a.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                cover: a.cover,
                detail: "Album".into(),
                album: String::new(),
            })
            .collect())
    } else {
        let list: tidalrs::List<Track> = serde_json::from_str(&body)?;
        Ok(track_items(&list.items))
    }
}

fn art_cache() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".cache"))
        .join("tidal-rs/artwork")
}

fn request_art(
    keys: Vec<String>,
    pending: &mut HashSet<String>,
    http: &reqwest::Client,
    limit: &Arc<Semaphore>,
    auth: Option<Arc<Authz>>,
    country: String,
    tx: &mpsc::Sender<Event>,
) {
    for key in keys {
        if (!(key.len() == 36 && key.bytes().all(|c| c.is_ascii_hexdigit() || c == b'-'))
            && !key.starts_with("section:pages/"))
            || !pending.insert(key.clone())
        {
            continue;
        }
        let auth = auth.clone();
        let country = country.clone();
        let http = http.clone();
        let limit = limit.clone();
        let tx = tx.clone();
        tokio::spawn(async move {
            let Ok(_permit) = limit.acquire().await else {
                return;
            };
            let path = art_cache().join(format!("{}.jpg", key.replace(['/', ':'], "_")));
            if !path.exists() {
                let image_key = if let Some(section) = key.strip_prefix("section:") {
                    if section.contains("..") || section.contains(['?', '#', ':']) {
                        return;
                    }
                    let Some(auth) = auth else {
                        return;
                    };
                    let Ok(response) = http
                        .get(format!("https://api.tidal.com/v1/{section}"))
                        .bearer_auth(&auth.access_token)
                        .query(&[("countryCode", country.as_str()), ("deviceType", "BROWSER")])
                        .send()
                        .await
                    else {
                        return;
                    };
                    let Ok(body) = response.text().await else {
                        return;
                    };
                    let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) else {
                        return;
                    };
                    let Some(cover) = discovery_items(&value)
                        .into_iter()
                        .filter_map(|i| i.cover)
                        .find(|k| !k.starts_with("section:"))
                    else {
                        return;
                    };
                    cover
                } else {
                    key.clone()
                };
                if image_key.len() != 36
                    || !image_key
                        .bytes()
                        .all(|c| c.is_ascii_hexdigit() || c == b'-')
                {
                    return;
                }
                let url = format!(
                    "https://resources.tidal.com/images/{}/320x320.jpg",
                    image_key.replace('-', "/")
                );
                let Ok(response) = http.get(url).send().await else {
                    return;
                };
                if !response.status().is_success()
                    || response.content_length().is_some_and(|n| n > 2_000_000)
                {
                    return;
                }
                let Ok(bytes) = response.bytes().await else {
                    return;
                };
                if bytes.len() > 2_000_000 {
                    return;
                }
                if fs::create_dir_all(art_cache()).is_err() {
                    return;
                }
                let temp = path.with_extension(format!("{}.tmp", std::process::id()));
                if fs::write(&temp, &bytes).is_err() {
                    return;
                }
                if fs::rename(temp, &path).is_err() {
                    return;
                }
            }
            let _ = tx.send(Event::Artwork(key, path));
        });
    }
}

fn quality_name(quality: AudioQuality) -> &'static str {
    match quality {
        AudioQuality::Low => "LOW",
        AudioQuality::High => "HIGH",
        AudioQuality::Lossless => "LOSSLESS",
        AudioQuality::HiResLossless => "HI-RES",
    }
}
fn playback(library: &Library, player: &mut Player) -> Option<Playback> {
    let track = library.tracks.get(library.current?)?;
    Some(Playback {
        can_next: library
            .current
            .is_some_and(|i| i + 1 < library.tracks.len()),
        can_previous: library.current.is_some_and(|i| i > 0),
        album_title: track.album.title.clone(),
        album_id: track.album.id,
        requested: quality_name(library.quality).into(),
        codec: {
            let codec = player
                .property("audio-codec-name")
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default();
            let rate = player
                .property("audio-params/samplerate")
                .ok()
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            if rate > 0 {
                format!("{codec} / {:.1} kHz", rate as f64 / 1000.0)
            } else {
                codec
            }
        },
        title: track.title.clone(),
        artist: artists(track),
        cover: track.album.cover.clone(),
        position: player.property("time-pos").ok()?.as_f64().unwrap_or(0.0),
        duration: f64::from(track.duration),
        paused: player.property("pause").ok()?.as_bool().unwrap_or(true),
        quality: {
            let actual = library.actual_quality.unwrap_or(library.quality);
            if actual < library.quality {
                format!("{} ↓", quality_name(actual))
            } else {
                quality_name(actual).into()
            }
        },
    })
}

pub struct Shutdown {
    cancel: Option<tokio::sync::oneshot::Sender<()>>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Drop for Shutdown {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub fn spawn() -> (
    channel::UnboundedSender<Action>,
    mpsc::Receiver<Event>,
    Shutdown,
) {
    let (actions, mut rx) = channel::unbounded_channel();
    let (tx, events) = mpsc::channel();
    let (cancel, cancelled) = tokio::sync::oneshot::channel();
    let worker = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("async runtime");
        runtime.block_on(async move {
            let work=async move {
            let directory=match online::state_directory(){Ok(p)=>p,Err(e)=>{let _=tx.send(Event::Error(e.to_string()));return;}};
            let token_path=directory.join("session.json");
            let mut client=online::client(&token_path).ok().and_then(|c|{
                fs::read(&token_path).ok().and_then(|data|serde_json::from_slice::<Authz>(&data).ok()).map(|auth|c.with_authz(auth))
            });
            let _=tx.send(if client.is_some(){Event::Ready}else{Event::NeedsLogin});
            let mut player:Option<Player>=None;
            let preferences=Preferences::load();let mut choice=preferences.quality;
            let mut library=Library::new();library.quality=choice.audio();
            let mut history=History::load(client.as_ref().and_then(|c|c.get_user_id()));
            let mut saved_ids=None::<HashSet<u64>>;let mut saved_at=std::time::Instant::now();
            let mut listing=vec![];let mut listing_epoch=0;let mut queue_epoch=None;let mut volume=preferences.volume;
            let http=reqwest::Client::builder().timeout(Duration::from_secs(12)).build().unwrap();
            let mut art_pending=HashSet::new();let art_limit=Arc::new(Semaphore::new(6));
            let mut tick=tokio::time::interval(Duration::from_millis(700));
            let mut spectrum=None;
            loop {
                tokio::select! {
                    action=rx.recv()=> {
                        let Some(action)=action else{break;};
                        if matches!(action,Action::Shutdown){break;}
                        let mut record_started=false;
                        let outcome:Result<(),Box<dyn std::error::Error>>=async {
                            match action {
                                Action::Login=> {
                                    let send=tx.clone();
                                    let c=online::login_with(&token_path,move |url,code|{let _=send.send(Event::LoginCode(url.into(),code.into()));}).await?;
                                    client=Some(c);saved_ids=None;history=History::load(client.as_ref().and_then(|c|c.get_user_id()));let _=tx.send(Event::Ready);
                                },
                                Action::Logout=>{spectrum=None;player=None;library=Library::new();library.quality=choice.audio();saved_ids=None;client=None;if token_path.exists(){fs::remove_file(&token_path)?;}let _=tx.send(Event::NeedsLogin);},
                                Action::Browse{view,page,epoch}=>{
                                    let c=client.as_ref().ok_or("Please sign in to your account")?;
                                    if saved_ids.is_none()||saved_at.elapsed()>Duration::from_secs(60){
                                        let ids=favorite_ids(c).await?;let _=tx.send(Event::SavedAlbums(ids.iter().copied().collect()));saved_ids=Some(ids);saved_at=std::time::Instant::now();
                                    }
                                    let append=page>1&&listing_epoch==epoch;
                                    let (mut page,tracks)=fetch(c,&view,page,epoch).await?;
                                    request_art(page.items.iter().filter_map(|i|i.cover.clone()).collect(),&mut art_pending,&http,&art_limit,c.get_authz(),c.get_country_code(),&tx);
                                    if append {
                                        let base=listing.len();for item in &mut page.items {if let Key::Track(i)=&mut item.key{*i+=base;}}
                                        if queue_epoch==Some(epoch){library.tracks.extend(tracks.clone());let _=tx.send(Event::Queue(track_items(&library.tracks)));}
                                        listing.extend(tracks);
                                    }else{listing=tracks;}
                                    listing_epoch=epoch;let _=tx.send(Event::Page(page));
                                },
                                Action::Play{index,epoch}=>{
                                    if epoch!=listing_epoch{return Err("The library page changed. Select the track again.".into());}
                                    if index>=listing.len(){return Err("Select a track first".into());}
                                    if player.is_none(){
                                        let mut p=Player::start(std::env::var_os("TIDAL_GUI_SILENT").is_some())?;
                                        p.command(json!(["set_property","volume",volume]))?;player=Some(p);
                                    }
                                    if spectrum.is_none(){spectrum=Some(super::spectrum::Monitor::start());}
                                    library.tracks=listing.clone();library.current=None;queue_epoch=Some(epoch);
                                    let _=tx.send(Event::Queue(track_items(&library.tracks)));
                                    let _=tx.send(Event::LoadingTrack(library.tracks[index].title.clone()));
                                    library.play(client.as_ref().ok_or("Please sign in")?,player.as_mut().unwrap(),index).await?;record_started=true;
                                },
                                Action::Suggest{query,generation,albums}=>{
                                    if let Some(c)=client.as_ref(){if let Some(auth)=c.get_authz(){
                                        let tx=tx.clone();let country=c.get_country_code();
                                        tokio::spawn(async move{let items=suggestions(&auth,&country,&query,albums).await.unwrap_or_default();let _=tx.send(Event::Suggestions{query,generation,items});});
                                    }}
                                },
                                Action::PlayQueued(index)=>{
                                    if index>=library.tracks.len(){return Err("Queue changed".into());}
                                    let _=tx.send(Event::LoadingTrack(library.tracks[index].title.clone()));
                                    library.play(client.as_ref().ok_or("Please sign in")?,player.as_mut().ok_or("Player unavailable")?,index).await?;record_started=true;
                                },
                                Action::FavoriteAlbum{id,saved}=>{
                                    let c=client.as_ref().ok_or("Please sign in")?;
                                    if saved{c.add_favorite_album(id).await?;}else{c.remove_favorite_album(id).await?;}
                                    if let Some(ids)=saved_ids.as_mut(){if saved{ids.insert(id);}else{ids.remove(&id);}}
                                    let _=tx.send(Event::AlbumSaved(id,saved));
                                },
                                Action::Quality(index)=>{
                                    choice=Quality::from_index(index);library.quality=choice.audio();
                                    if let (Some(index),Some(p),Some(c))=(library.current,player.as_mut(),client.as_ref()){
                                        let position=p.property("time-pos").ok().and_then(|v|v.as_f64()).unwrap_or(0.0);
                                        let paused=p.property("pause").ok().and_then(|v|v.as_bool()).unwrap_or(false);
                                        library.play(c,p,index).await?;
                                        p.command(json!(["seek",position,"absolute+exact"]))?;
                                        p.command(json!(["set_property","pause",paused]))?;
                                    }
                                },
                                Action::Toggle=>{if let Some(p)=player.as_mut(){p.command(json!(["cycle","pause"]))?;}},
                                Action::Seek(position)=>{if let Some(p)=player.as_mut(){p.command(json!(["seek",position.max(0.0),"absolute+exact"]))?;}},
                                Action::Volume(value)=>{volume=value.clamp(0.0,100.0);if let Some(p)=player.as_mut(){p.command(json!(["set_property","volume",volume]))?;}},
                                Action::Next|Action::Previous=>{
                                    let current=library.current.ok_or("Choose a track to start listening")?;
                                    let index=if matches!(action,Action::Next){current+1}else{current.saturating_sub(1)};
                                    if index>=library.tracks.len(){return Err("End of the current track page".into());}
                                    library.current=None;
                                    let _=tx.send(Event::LoadingTrack(library.tracks[index].title.clone()));
                                    library.play(client.as_ref().ok_or("Please sign in")?,player.as_mut().ok_or("Player unavailable")?,index).await?;record_started=true;
                                },
                                Action::Shutdown=>{},
                            }
                            Ok(())
                        }.await;
                        if let Err(e)=outcome {let _=tx.send(Event::Error(e.to_string()));}
                        else if record_started {if let Some(track)=library.current.and_then(|i|library.tracks.get(i)){if let Err(e)=history.record(track){let _=tx.send(Event::Error(format!("Could not save listening history: {e}")));}}}
                    },
                    _=tick.tick()=>{
                        if let Some(p)=player.as_mut(){
                            if let Some(index)=library.current {
                                if p.property("eof-reached").ok().and_then(|v|v.as_bool())==Some(true)&&index+1<library.tracks.len(){
                                    library.current=None;let _=tx.send(Event::LoadingTrack(library.tracks[index+1].title.clone()));
                                    if let Some(c)=client.as_ref(){match library.play(c,p,index+1).await{Err(e)=>{let _=tx.send(Event::Error(e.to_string()));},Ok(())=>{let _=history.record(&library.tracks[index+1]);}}}
                                }
                            }
                            if let Some(state)=playback(&library,p){
                                request_art(state.cover.clone().into_iter().collect(),&mut art_pending,&http,&art_limit,client.as_ref().and_then(|c|c.get_authz()),client.as_ref().map(|c|c.get_country_code()).unwrap_or_default(),&tx);
                                let _=tx.send(Event::Playback(state));
                            }
                        }
                    }
                }
            }
            };
            tokio::select!{_=work=>{},_=cancelled=>{}}
        });
    });
    (
        actions,
        events,
        Shutdown {
            cancel: Some(cancel),
            worker: Some(worker),
        },
    )
}

fn discovery_items(value: &serde_json::Value) -> Vec<Item> {
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    if let Some(rows) = value["rows"].as_array() {
        for row in rows {
            if let Some(modules) = row["modules"].as_array() {
                for module in modules {
                    let section = module["title"].as_str().unwrap_or("");
                    if let Some(items) = module["pagedList"]["items"].as_array() {
                        for item in items {
                            let text = |key: &str| item[key].as_str().unwrap_or("").to_owned();
                            let title = text("title");
                            if title.is_empty() {
                                continue;
                            }
                            let kind = module["type"].as_str().unwrap_or("");
                            let (key, cover) = if kind == "ALBUM_LIST" {
                                let Some(id) = item["id"].as_u64() else {
                                    continue;
                                };
                                (Key::Album(id), item["cover"].as_str().map(str::to_owned))
                            } else if kind == "PLAYLIST_LIST" {
                                (
                                    Key::Playlist(text("uuid")),
                                    item["squareImage"]
                                        .as_str()
                                        .or(item["image"].as_str())
                                        .map(str::to_owned),
                                )
                            } else if kind.starts_with("PAGE_LINKS")
                                && text("apiPath").starts_with("pages/")
                            {
                                (
                                    Key::Discovery(text("apiPath")),
                                    Some(format!("section:{}", text("apiPath"))),
                                )
                            } else {
                                continue;
                            };
                            if !seen.insert(format!("{key:?}")) {
                                continue;
                            }
                            let names = item["artists"]
                                .as_array()
                                .map(|a| {
                                    a.iter()
                                        .filter_map(|x| x["name"].as_str())
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                })
                                .unwrap_or_default();
                            result.push(Item {
                                key,
                                title,
                                subtitle: if names.is_empty() {
                                    section.into()
                                } else {
                                    names
                                },
                                cover,
                                detail: section.into(),
                                album: String::new(),
                            });
                        }
                    }
                }
            }
        }
    }
    result
}

async fn favorite_ids(client: &TidalClient) -> Result<HashSet<u64>, Box<dyn std::error::Error>> {
    let mut result = HashSet::new();
    let mut offset = 0;
    loop {
        let list = client
            .favorite_albums(Some(offset), Some(1000), None, None)
            .await?;
        let count = list.items.len();
        result.extend(list.items.into_iter().map(|a| a.item.id));
        offset += count as u32;
        if offset as usize >= list.total {
            break;
        }
        if count == 0 {
            return Err("Saved album list was incomplete".into());
        }
    }
    Ok(result)
}
fn album_item(a: tidalrs::Album, detail: String) -> Item {
    Item {
        key: Key::Album(a.id),
        title: a.title,
        subtitle: a
            .artists
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        cover: a.cover,
        detail,
        album: String::new(),
    }
}
async fn home(
    client: &TidalClient,
) -> Result<(Vec<(String, Vec<Item>)>, Vec<Track>), Box<dyn std::error::Error>> {
    let history = History::load(client.get_user_id());
    let tracks: Vec<_> = history
        .entries
        .iter()
        .take(12)
        .map(|e| e.track.clone())
        .collect();
    let mut groups = vec![("Recently played songs".into(), track_items(&tracks))];
    let mut seen = HashSet::new();
    let albums = history
        .entries
        .iter()
        .filter(|e| seen.insert(e.track.album.id))
        .take(12)
        .map(|e| Item {
            key: Key::Album(e.track.album.id),
            title: e.track.album.title.clone(),
            subtitle: artists(&e.track),
            cover: e.track.album.cover.clone(),
            detail: "Recently played".into(),
            album: String::new(),
        })
        .collect();
    groups.push(("Recently played albums".into(), albums));
    let favorites = client
        .favorite_artists(Some(0), Some(6), None, None)
        .await?;
    let mut artists: Vec<(u64, String)> = favorites
        .items
        .into_iter()
        .map(|a| (a.item.id, a.item.name))
        .collect();
    if artists.is_empty() {
        let favorites = client
            .favorite_tracks(Some(0), Some(100), None, None)
            .await?;
        let mut counts = std::collections::HashMap::<u64, (String, usize)>::new();
        for favorite in favorites.items {
            for a in favorite.item.artists {
                let entry = counts.entry(a.id).or_insert((a.name, 0));
                entry.1 += 1;
            }
        }
        let mut ranked: Vec<_> = counts.into_iter().collect();
        ranked.sort_by(|a, b| b.1.1.cmp(&a.1.1).then(a.0.cmp(&b.0)));
        artists = ranked
            .into_iter()
            .take(6)
            .map(|(id, (name, _))| (id, name))
            .collect();
    }
    if artists.is_empty() {
        let albums = client
            .favorite_albums(Some(0), Some(12), None, None)
            .await?;
        let mut ids = HashSet::new();
        for album in albums.items {
            for a in album.item.artists {
                if ids.insert(a.id) {
                    artists.push((a.id, a.name));
                }
            }
        }
        artists.truncate(6);
    }
    for (id, name) in artists {
        if let Ok(list) = client.artist_albums(id, None, Some(0), Some(6)).await {
            let mut albums = list.items;
            albums.sort_by(|a, b| b.release_date.cmp(&a.release_date));
            groups.push((
                format!("From your favorites · {name}"),
                albums
                    .into_iter()
                    .take(4)
                    .map(|a| album_item(a, format!("From {name}")))
                    .collect(),
            ));
        }
    }
    if let Ok(value) = discovery_page(client, "pages/home", 0).await {
        let mut items = discovery_items(&value);
        items.truncate(12);
        groups.push(("From TIDAL".into(), items));
    }
    Ok((groups, tracks))
}
