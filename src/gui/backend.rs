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
    Explore,
    Discovery { path: String, title: String },
    SearchAlbums(String),
    Favorites,
    Albums,
    Playlists,
    Album { id: u64, title: String },
    Playlist { id: String, title: String },
    Search(String),
}
impl View {
    pub fn title(&self) -> String {
        match self {
            Self::Explore => "Explore".into(),
            Self::Discovery { title, .. } => title.clone(),
            Self::SearchAlbums(q) => format!("Albums for “{q}”"),
            Self::Favorites => "Favorite tracks".into(),
            Self::Albums => "Your albums".into(),
            Self::Playlists => "Your playlists".into(),
            Self::Album { title, .. } | Self::Playlist { title, .. } => title.clone(),
            Self::Search(q) => format!("Results for “{q}”"),
        }
    }
    pub fn is_grid(&self) -> bool {
        matches!(
            self,
            Self::Albums
                | Self::Playlists
                | Self::Explore
                | Self::Discovery { .. }
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
    pub items: Vec<Item>,
}
#[derive(Clone)]
pub struct Playback {
    pub title: String,
    pub artist: String,
    pub cover: Option<String>,
    pub position: f64,
    pub duration: f64,
    pub paused: bool,
    pub quality: String,
    pub level: f64,
    pub can_next: bool,
    pub can_previous: bool,
}
pub enum Action {
    Browse { view: View, page: u32, epoch: u64 },
    Play { index: usize, epoch: u64 },
    SaveAlbum { id: u64 },
    Toggle,
    Next,
    Previous,
    Seek(f64),
    Volume(f64),
    Quality(bool),
    Login,
    Logout,
    Shutdown,
}
pub enum Event {
    Level(f64),
    AlbumSaved(u64),
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
    let (items, total, tracks) = match view {
        View::Explore | View::Discovery { .. } => {
            // Refresh through the SDK before using the pages endpoint it does not expose.
            client.favorite_tracks(Some(0), Some(1), None, None).await?;
            let path = match view {
                View::Discovery { path, .. } => path.as_str(),
                _ => "pages/explore",
            };
            if !path.starts_with("pages/") || path.contains("..") || path.contains(['?', '#', ':'])
            {
                return Err("Invalid discovery path".into());
            }
            let auth = client.get_authz().ok_or("Please sign in")?;
            let response = reqwest::Client::builder()
                .timeout(Duration::from_secs(25))
                .build()?
                .get(format!("https://api.tidal.com/v1/{path}"))
                .bearer_auth(&auth.access_token)
                .query(&[
                    ("countryCode", client.get_country_code()),
                    ("deviceType", "BROWSER".into()),
                    ("locale", "en_US".into()),
                ])
                .send()
                .await?
                .error_for_status()?
                .text()
                .await?;
            let value: serde_json::Value = serde_json::from_str(&response)?;
            let all = discovery_items(&value);
            let total = all.len();
            (
                all.into_iter()
                    .skip(offset as usize)
                    .take(PAGE_SIZE as usize)
                    .collect(),
                total,
                vec![],
            )
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
            items,
        },
        tracks,
    ))
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
    tx: &mpsc::Sender<Event>,
) {
    for key in keys {
        if key.len() != 36
            || !key.bytes().all(|c| c.is_ascii_hexdigit() || c == b'-')
            || !pending.insert(key.clone())
        {
            continue;
        }
        let http = http.clone();
        let limit = limit.clone();
        let tx = tx.clone();
        tokio::spawn(async move {
            let Ok(_permit) = limit.acquire().await else {
                return;
            };
            let path = art_cache().join(format!("{key}.jpg"));
            if !path.exists() {
                let url = format!(
                    "https://resources.tidal.com/images/{}/320x320.jpg",
                    key.replace('-', "/")
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

fn audio_level(player: &mut Player) -> f64 {
    player
        .property("af-metadata/meter")
        .ok()
        .and_then(|v| {
            v.get("lavfi.astats.Overall.RMS_level")
                .and_then(|x| x.as_str())
                .and_then(|x| x.parse::<f64>().ok())
        })
        .filter(|db| db.is_finite())
        .map(|db| ((db + 60.0) / 60.0).clamp(0.0, 1.0))
        .unwrap_or(0.0)
}

fn playback(library: &Library, player: &mut Player) -> Option<Playback> {
    let track = library.tracks.get(library.current?)?;
    Some(Playback {
        can_next: library
            .current
            .is_some_and(|i| i + 1 < library.tracks.len()),
        can_previous: library.current.is_some_and(|i| i > 0),
        level: audio_level(player),
        title: track.title.clone(),
        artist: artists(track),
        cover: track.album.cover.clone(),
        position: player.property("time-pos").ok()?.as_f64().unwrap_or(0.0),
        duration: f64::from(track.duration),
        paused: player.property("pause").ok()?.as_bool().unwrap_or(true),
        quality: format!("{:?}", library.actual_quality.unwrap_or(library.quality)).to_uppercase(),
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
            let mut library=Library::new();let mut listing=vec![];let mut listing_epoch=0;let mut volume=35.0;
            let http=reqwest::Client::builder().timeout(Duration::from_secs(12)).build().unwrap();
            let mut art_pending=HashSet::new();let art_limit=Arc::new(Semaphore::new(6));
            let mut tick=tokio::time::interval(Duration::from_millis(700));
            let mut meter=tokio::time::interval(Duration::from_millis(80));
            loop {
                tokio::select! {
                    action=rx.recv()=> {
                        let Some(action)=action else{break;};
                        if matches!(action,Action::Shutdown){break;}
                        let outcome:Result<(),Box<dyn std::error::Error>>=async {
                            match action {
                                Action::Login=> {
                                    let send=tx.clone();
                                    let c=online::login_with(&token_path,move |url,code|{let _=send.send(Event::LoginCode(url.into(),code.into()));}).await?;
                                    client=Some(c);let _=tx.send(Event::Ready);
                                },
                                Action::Logout=>{player=None;library=Library::new();client=None;if token_path.exists(){fs::remove_file(&token_path)?;}let _=tx.send(Event::NeedsLogin);},
                                Action::Browse{view,page,epoch}=>{
                                    let c=client.as_ref().ok_or("Please sign in to your account")?;
                                    let (page,tracks)=fetch(c,&view,page,epoch).await?;
                                    request_art(page.items.iter().filter_map(|i|i.cover.clone()).collect(),&mut art_pending,&http,&art_limit,&tx);
                                    listing=tracks;listing_epoch=epoch;let _=tx.send(Event::Page(page));
                                },
                                Action::Play{index,epoch}=>{
                                    if epoch!=listing_epoch{return Err("The library page changed. Select the track again.".into());}
                                    if index>=listing.len(){return Err("Select a track first".into());}
                                    if player.is_none(){
                                        let mut p=Player::start(std::env::var_os("TIDAL_GUI_SILENT").is_some())?;
                                        p.command(json!(["set_property","volume",volume]))?;player=Some(p);
                                    }
                                    library.tracks=listing.clone();library.current=None;
                                    let _=tx.send(Event::LoadingTrack(library.tracks[index].title.clone()));
                                    library.play(client.as_ref().ok_or("Please sign in")?,player.as_mut().unwrap(),index).await?;
                                },
                                Action::SaveAlbum{id}=>{
                                    client.as_ref().ok_or("Please sign in")?.add_favorite_album(id).await?;
                                    let _=tx.send(Event::AlbumSaved(id));
                                },
                                Action::Quality(lossless)=>{library.quality=if lossless{AudioQuality::Lossless}else{AudioQuality::High};},
                                Action::Toggle=>{if let Some(p)=player.as_mut(){p.command(json!(["cycle","pause"]))?;}},
                                Action::Seek(position)=>{if let Some(p)=player.as_mut(){p.command(json!(["seek",position.max(0.0),"absolute+exact"]))?;}},
                                Action::Volume(value)=>{volume=value.clamp(0.0,100.0);if let Some(p)=player.as_mut(){p.command(json!(["set_property","volume",volume]))?;}},
                                Action::Next|Action::Previous=>{
                                    let current=library.current.ok_or("Choose a track to start listening")?;
                                    let index=if matches!(action,Action::Next){current+1}else{current.saturating_sub(1)};
                                    if index>=library.tracks.len(){return Err("End of the current track page".into());}
                                    library.current=None;
                                    let _=tx.send(Event::LoadingTrack(library.tracks[index].title.clone()));
                                    library.play(client.as_ref().ok_or("Please sign in")?,player.as_mut().ok_or("Player unavailable")?,index).await?;
                                },
                                Action::Shutdown=>{},
                            }
                            Ok(())
                        }.await;
                        if let Err(e)=outcome {let _=tx.send(Event::Error(e.to_string()));}
                    },
                    _=meter.tick()=>{
                        if library.current.is_some(){if let Some(p)=player.as_mut(){let _=tx.send(Event::Level(audio_level(p)));}}
                    },
                    _=tick.tick()=>{
                        if let Some(p)=player.as_mut(){
                            if let Some(index)=library.current {
                                if p.property("eof-reached").ok().and_then(|v|v.as_bool())==Some(true)&&index+1<library.tracks.len(){
                                    library.current=None;let _=tx.send(Event::LoadingTrack(library.tracks[index+1].title.clone()));
                                    if let Some(c)=client.as_ref(){if let Err(e)=library.play(c,p,index+1).await{let _=tx.send(Event::Error(e.to_string()));}}
                                }
                            }
                            if let Some(state)=playback(&library,p){
                                request_art(state.cover.clone().into_iter().collect(),&mut art_pending,&http,&art_limit,&tx);
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
                                (Key::Discovery(text("apiPath")), None)
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
