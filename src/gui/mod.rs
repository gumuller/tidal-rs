mod artwork;
mod backend;
mod visualizer;
use crate::theme::Theme;
use backend::{Action, Event, Item, Key, PAGE_SIZE, Page, View};
use gtk::{gdk, gio, glib, pango, prelude::*};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    path::PathBuf,
    rc::Rc,
    time::Duration,
};
use tokio::sync::mpsc::UnboundedSender;

#[derive(Clone)]
struct Cover {
    picture: artwork::Artwork,
    icon: gtk::Image,
}
struct State {
    epoch: u64,
    view: View,
    page: u32,
    history: Vec<(View, u32)>,
    items: Vec<Item>,
    art: HashMap<String, PathBuf>,
    covers: HashMap<String, Vec<Cover>>,
    now_cover: Option<(String, Cover)>,
    last_playback: Option<backend::Playback>,
    theme_css: String,
}
#[derive(Clone)]
struct Ui {
    window: gtk::ApplicationWindow,
    title: gtk::Label,
    toolbar: gtk::Box,
    discovery_filters: gtk::Box,
    hero: gtk::Box,
    hero_art: gtk::Box,
    hero_title: gtk::Label,
    hero_artist: gtk::Label,
    save_album: gtk::Button,
    visualizer: gtk::DrawingArea,
    subtitle: gtk::Label,
    theme: gtk::Label,
    stack: gtk::Stack,
    content: gtk::Stack,
    grid: gtk::FlowBox,
    tracks: gtk::ListBox,
    error: gtk::Revealer,
    error_text: gtk::Label,
    previous: gtk::Button,
    next: gtk::Button,
    page: gtk::Label,
    pagination: gtk::Box,
    back: gtk::Button,
    nav: Vec<gtk::Button>,
    account: gtk::Label,
    login_button: gtk::Button,
    login_detail: gtk::Label,
    sign_in: gtk::Button,
    sign_out: gtk::Button,
    play: gtk::Button,
    previous_track: gtk::Button,
    next_track: gtk::Button,
    now_title: gtk::Label,
    now_artist: gtk::Label,
    now_art: gtk::Box,
    seek: gtk::Scale,
    elapsed: gtk::Label,
    duration: gtk::Label,
    quality: gtk::Label,
    search: gtk::SearchEntry,
    album_search: gtk::ToggleButton,
}

fn label(text: &str, class: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    if !class.is_empty() {
        l.add_css_class(class);
    }
    l
}
fn icon_button(icon: &str, tip: &str) -> gtk::Button {
    let button = gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tip)
        .build();
    button.update_property(&[gtk::accessible::Property::Label(tip)]);
    button
}
fn vbox(spacing: i32) -> gtk::Box {
    gtk::Box::new(gtk::Orientation::Vertical, spacing)
}
fn hbox(spacing: i32) -> gtk::Box {
    gtk::Box::new(gtk::Orientation::Horizontal, spacing)
}
fn time(seconds: f64) -> String {
    let n = seconds.max(0.0) as u64;
    format!("{}:{:02}", n / 60, n % 60)
}
fn send(tx: &UnboundedSender<Action>, action: Action) {
    let _ = tx.send(action);
}

fn set_cover(cover: &Cover, path: &std::path::Path) {
    if let Ok(texture) = gdk::Texture::from_filename(path) {
        cover.picture.set_texture(&texture);
        cover.icon.set_visible(false);
    }
}
fn cover(key: Option<&str>, size: i32, state: &Rc<RefCell<State>>) -> (gtk::Overlay, Cover) {
    let picture = artwork::Artwork::new(size);
    let icon = gtk::Image::from_icon_name("audio-x-generic-symbolic");
    icon.set_pixel_size(if size > 100 { 40 } else { 20 });
    icon.add_css_class("cover-placeholder");
    let frame = gtk::Overlay::new();
    frame.set_child(Some(&picture));
    frame.add_overlay(&icon);
    frame.set_halign(gtk::Align::Start);
    let art = Cover { picture, icon };
    if let Some(key) = key {
        let mut s = state.borrow_mut();
        if let Some(path) = s.art.get(key) {
            set_cover(&art, path);
        }
        s.covers.entry(key.into()).or_default().push(art.clone());
    }
    (frame, art)
}

fn navigate(
    ui: &Ui,
    state: &Rc<RefCell<State>>,
    tx: &UnboundedSender<Action>,
    view: View,
    page: u32,
    history: bool,
) {
    let epoch = {
        let mut s = state.borrow_mut();
        if history {
            let last = (s.view.clone(), s.page);
            s.history.push(last);
        }
        s.view = view.clone();
        s.page = page;
        s.epoch += 1;
        s.epoch
    };
    ui.title.set_text(&view.title());
    ui.title.update_state(&[gtk::accessible::State::Busy(true)]);
    ui.subtitle.set_text("Loading your collection…");
    ui.stack.set_visible_child_name("loading");
    ui.error.set_reveal_child(false);
    ui.previous.set_sensitive(false);
    ui.next.set_sensitive(false);
    ui.back.set_sensitive(!state.borrow().history.is_empty());
    for (index, button) in ui.nav.iter().enumerate() {
        let active = matches!(
            (&view, index),
            (View::Albums, 0) | (View::Favorites, 1) | (View::Playlists, 2) | (View::Explore, 3)
        );
        if active {
            button.add_css_class("active");
        } else {
            button.remove_css_class("active");
        }
    }
    send(tx, Action::Browse { view, page, epoch });
}

fn activate(ui: &Ui, state: &Rc<RefCell<State>>, tx: &UnboundedSender<Action>, index: usize) {
    let item = state.borrow().items.get(index).cloned();
    let Some(item) = item else {
        return;
    };
    match item.key {
        Key::Discovery(path) => navigate(
            ui,
            state,
            tx,
            View::Discovery {
                path,
                section: None,
                title: item.title,
            },
            1,
            true,
        ),
        Key::Track(index) => send(
            tx,
            Action::Play {
                index,
                epoch: state.borrow().epoch,
            },
        ),
        Key::Album(id) => navigate(
            ui,
            state,
            tx,
            View::Album {
                id,
                title: item.title,
            },
            1,
            true,
        ),
        Key::Playlist(id) => navigate(
            ui,
            state,
            tx,
            View::Playlist {
                id,
                title: item.title,
            },
            1,
            true,
        ),
    }
}

fn display(ui: &Ui, state: &Rc<RefCell<State>>, tx: &UnboundedSender<Action>, page: Page) {
    if page.epoch != state.borrow().epoch {
        return;
    }
    while let Some(child) = ui.grid.first_child() {
        ui.grid.remove(&child);
    }
    while let Some(child) = ui.tracks.first_child() {
        ui.tracks.remove(&child);
    }
    state.borrow_mut().covers.clear();
    while let Some(child) = ui.discovery_filters.first_child() {
        ui.discovery_filters.remove(&child);
    }
    ui.discovery_filters.set_visible(!page.sections.is_empty());
    if let View::Discovery {
        path,
        title,
        section: selected,
    } = &page.view
    {
        for section in std::iter::once("All".to_owned()).chain(page.sections.iter().cloned()) {
            let button = gtk::Button::with_label(&section);
            let section = if section == "All" {
                None
            } else {
                Some(section)
            };
            if &section == selected {
                button.add_css_class("active");
            }
            let view = View::Discovery {
                path: path.clone(),
                title: title.clone(),
                section,
            };
            let ui = ui.clone();
            let state = state.clone();
            let tx = tx.clone();
            ui.discovery_filters.append(&button);
            button.connect_clicked(move |_| navigate(&ui, &state, &tx, view.clone(), 1, true));
        }
    }
    let grid = page.view.is_grid();
    let album = matches!(&page.view, View::Album { .. });
    ui.hero.set_visible(album);
    ui.save_album.set_label("Save album");
    ui.save_album.set_sensitive(true);
    while let Some(child) = ui.hero_art.first_child() {
        ui.hero_art.remove(&child);
    }
    if album {
        let first = page.items.first();
        let (art, _) = cover(first.and_then(|i| i.cover.as_deref()), 220, state);
        ui.hero_art.append(&art);
        ui.hero_title.set_text(&page.view.title());
        ui.hero_artist
            .set_text(first.map(|i| i.subtitle.as_str()).unwrap_or(""));
    }
    for (index, item) in page.items.iter().enumerate() {
        if grid {
            let card = vbox(0);
            card.add_css_class("album-card");
            card.set_width_request(164);
            card.set_halign(gtk::Align::Start);
            let (art, slot) = cover(item.cover.as_deref(), 164, state);
            if matches!(item.key, Key::Discovery(_)) {
                slot.icon.set_icon_name(Some("find-location-symbolic"));
            }
            card.append(&art);
            let title = label(&item.title, "card-title");
            title.set_ellipsize(pango::EllipsizeMode::End);
            title.set_width_chars(1);
            title.set_max_width_chars(18);
            title.set_tooltip_text(Some(&item.title));
            let artist = label(&item.subtitle, "card-artist");
            artist.set_ellipsize(pango::EllipsizeMode::End);
            artist.set_width_chars(1);
            artist.set_max_width_chars(18);
            card.append(&title);
            card.append(&artist);
            if matches!(page.view, View::Discovery { .. }) && !item.detail.is_empty() {
                let category = label(&item.detail, "eyebrow");
                category.set_margin_top(7);
                category.set_ellipsize(pango::EllipsizeMode::End);
                category.set_width_chars(1);
                category.set_max_width_chars(18);
                card.append(&category);
            }
            let button = gtk::Button::new();
            button.add_css_class("album-card-button");
            button.set_halign(gtk::Align::Start);
            button.set_child(Some(&card));
            button.update_property(&[gtk::accessible::Property::Label(&format!(
                "Open {}",
                item.title
            ))]);
            {
                let ui = ui.clone();
                let state = state.clone();
                let tx = tx.clone();
                button.connect_clicked(move |_| activate(&ui, &state, &tx, index));
            }
            ui.grid.insert(&button, -1);
        } else {
            let row = hbox(14);
            let number = label(&format!("{:02}", index + 1), "muted");
            number.add_css_class("small");
            number.set_width_chars(3);
            row.append(&number);
            let (art, _) = cover(item.cover.as_deref(), 42, state);
            row.append(&art);
            let names = vbox(4);
            names.set_hexpand(true);
            let title = label(&item.title, "track-title");
            title.set_ellipsize(pango::EllipsizeMode::End);
            title.set_max_width_chars(40);
            let artist = label(&item.subtitle, "muted");
            artist.add_css_class("small");
            artist.set_ellipsize(pango::EllipsizeMode::End);
            artist.set_max_width_chars(40);
            names.append(&title);
            names.append(&artist);
            row.append(&names);
            let album = label(&item.album, "muted");
            album.set_ellipsize(pango::EllipsizeMode::End);
            album.set_width_chars(22);
            album.set_max_width_chars(22);
            album.set_visible(!matches!(page.view, View::Album { .. }));
            row.append(&album);
            let duration = label(&item.detail, "muted");
            duration.add_css_class("small");
            duration.set_width_chars(5);
            duration.set_xalign(1.0);
            row.append(&duration);
            let play = gtk::Image::from_icon_name("media-playback-start-symbolic");
            play.set_pixel_size(14);
            play.add_css_class("muted");
            row.append(&play);
            let button = gtk::Button::new();
            button.add_css_class("track-button");
            button.set_child(Some(&row));
            button.update_property(&[gtk::accessible::Property::Label(&format!(
                "Play track: {}",
                item.title
            ))]);
            {
                let ui = ui.clone();
                let state = state.clone();
                let tx = tx.clone();
                button.connect_clicked(move |_| activate(&ui, &state, &tx, index));
            }
            ui.tracks.append(&button);
            if let Some(child) = ui.tracks.row_at_index(index as i32) {
                child.set_activatable(false);
            }
        }
    }
    let count = page.total;
    ui.title
        .update_state(&[gtk::accessible::State::Busy(false)]);
    let pages = if album {
        1
    } else {
        (count as u32).div_ceil(PAGE_SIZE).max(1)
    };
    let noun = match &page.view {
        View::Albums => "saved albums",
        View::SearchAlbums(_) => "albums",
        View::Discovery {
            section: Some(section),
            ..
        } if section.to_lowercase().contains("album") => "albums",
        View::Discovery {
            section: Some(section),
            ..
        } if section.to_lowercase().contains("track") => "tracks",
        View::Explore | View::Discovery { .. } => "discoveries",
        View::Playlists => "playlists",
        View::Favorites => "favorite tracks",
        _ => "tracks",
    };
    ui.subtitle.set_text(&format!("{count} {noun}"));
    ui.page.set_text(&format!("Page {} of {pages}", page.page));
    ui.previous.set_sensitive(page.page > 1);
    ui.next.set_sensitive(page.page < pages);
    ui.content
        .set_visible_child_name(if grid { "grid" } else { "tracks" });
    ui.stack.set_visible_child_name(if page.items.is_empty() {
        "empty"
    } else {
        "content"
    });
    ui.pagination.set_visible(!page.items.is_empty() && !album);
    state.borrow_mut().items = page.items;
}

fn playback(
    ui: &Ui,
    state: &Rc<RefCell<State>>,
    value: backend::Playback,
    updating: &Rc<Cell<bool>>,
) {
    let changed = state
        .borrow()
        .last_playback
        .as_ref()
        .is_none_or(|old| old.title != value.title || old.artist != value.artist);
    if changed {
        ui.now_title.set_text(&value.title);
        ui.now_artist.set_text(&value.artist);
        while let Some(child) = ui.now_art.first_child() {
            ui.now_art.remove(&child);
        }
        let (art, slot) = cover(value.cover.as_deref(), 48, state);
        ui.now_art.append(&art);
        state.borrow_mut().now_cover = value.cover.as_ref().map(|key| (key.clone(), slot));
    }
    ui.play.set_icon_name(if value.paused {
        "media-playback-start-symbolic"
    } else {
        "media-playback-pause-symbolic"
    });
    ui.play.set_sensitive(true);
    ui.previous_track.set_sensitive(value.can_previous);
    ui.next_track.set_sensitive(value.can_next);
    ui.seek.set_sensitive(true);
    updating.set(true);
    ui.seek.set_range(0.0, value.duration.max(1.0));
    ui.seek.set_value(value.position);
    updating.set(false);
    ui.elapsed.set_text(&time(value.position));
    ui.duration.set_text(&time(value.duration));
    ui.quality.set_text(&value.quality);
    state.borrow_mut().last_playback = Some(value);
}

pub fn run() {
    let id = std::env::var("TIDAL_APP_ID").unwrap_or_else(|_| "io.github.tidalrs.Player".into());
    let app = gtk::Application::builder().application_id(&id).build();
    app.connect_activate(|app| {
        if let Some(window) = app.active_window() {
            window.present();
        } else {
            build(app);
        }
    });
    // `gui` selects this interface, rather than being a GApplication option.
    app.run_with_args::<&str>(&[]);
}

fn build(app: &gtk::Application) {
    let (tx, events, shutdown) = backend::spawn();
    let shutdown = RefCell::new(Some(shutdown));
    app.connect_shutdown(move |_| drop(shutdown.borrow_mut().take()));
    let theme = Theme::load();
    let provider = gtk::CssProvider::new();
    provider.load_from_string(&theme.css());
    gtk::style_context_add_provider_for_display(
        &gdk::Display::default().expect("Wayland display"),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    let state = Rc::new(RefCell::new(State {
        epoch: 0,
        view: View::Albums,
        page: 1,
        history: vec![],
        items: vec![],
        art: HashMap::new(),
        covers: HashMap::new(),
        now_cover: None,
        last_playback: None,
        theme_css: theme.css(),
    }));
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("TIDAL Rust")
        .default_width(1180)
        .default_height(820)
        .build();
    if std::env::var_os("TIDAL_GUI_SILENT").is_some() {
        window.set_title(Some("TIDAL Rust — Test preview"));
    }
    window.set_size_request(860, 540);
    window.set_icon_name(Some("io.github.tidalrs.Player"));
    window.add_css_class("tidal-window");
    let root = vbox(0);
    window.set_child(Some(&root));
    let body = hbox(0);
    body.set_vexpand(true);
    root.append(&body);
    let sidebar = vbox(0);
    sidebar.set_width_request(192);
    sidebar.add_css_class("sidebar");
    body.append(&sidebar);
    let brand = label("TIDAL", "brand");
    brand.set_margin_start(12);
    sidebar.append(&brand);
    let native = label("RUST  /  LINUX", "eyebrow");
    native.set_margin_start(12);
    native.set_margin_top(7);
    native.set_margin_bottom(44);
    sidebar.append(&native);
    let library_label = label("YOUR LIBRARY", "eyebrow");
    library_label.set_margin_start(14);
    library_label.set_margin_bottom(16);
    sidebar.append(&library_label);
    let mut nav = vec![];
    for (icon, text) in [
        ("media-optical-symbolic", "Albums"),
        ("emblem-favorite-symbolic", "Favorite tracks"),
        ("view-list-symbolic", "Playlists"),
        ("find-location-symbolic", "Explore"),
    ] {
        let button = gtk::Button::new();
        button.add_css_class("side-nav");
        let row = hbox(12);
        row.append(&gtk::Image::from_icon_name(icon));
        row.append(&label(text, ""));
        button.set_child(Some(&row));
        sidebar.append(&button);
        nav.push(button);
    }
    let stretch = vbox(0);
    stretch.set_vexpand(true);
    sidebar.append(&stretch);
    let theme_label = label(&format!("●  {}", theme.name), "theme-chip");
    theme_label.set_tooltip_text(Some("Follows your Omarchy theme automatically"));
    sidebar.append(&theme_label);
    let account = label("Connecting…", "muted");
    account.add_css_class("small");
    account.set_margin_start(10);
    account.set_margin_top(20);
    sidebar.append(&account);
    let account_actions = hbox(4);
    account_actions.set_margin_top(6);
    let reconnect = gtk::Button::with_label("Sign in");
    reconnect.set_visible(false);
    reconnect.add_css_class("small");
    let logout = icon_button("system-log-out-symbolic", "Sign out of TIDAL");
    logout.set_sensitive(false);
    account_actions.append(&reconnect);
    account_actions.append(&logout);
    sidebar.append(&account_actions);
    let main = vbox(0);
    main.set_hexpand(true);
    body.append(&main);
    let header = hbox(14);
    header.add_css_class("header");
    let back = icon_button("go-previous-symbolic", "Back");
    back.set_sensitive(false);
    back.set_valign(gtk::Align::Start);
    header.append(&back);
    let heading = vbox(0);
    heading.set_hexpand(true);
    let title = label("Your albums", "page-title");
    title.set_accessible_role(gtk::AccessibleRole::Heading);
    title.update_property(&[gtk::accessible::Property::Level(1)]);
    title.set_ellipsize(pango::EllipsizeMode::End);
    heading.append(&title);
    let subtitle = label("Connecting to your music…", "subtitle");
    heading.append(&subtitle);
    header.append(&heading);
    let search = gtk::SearchEntry::new();
    search.add_css_class("tidal-search");
    search.update_property(&[gtk::accessible::Property::Label("Search TIDAL catalog")]);
    search.set_placeholder_text(Some("Search TIDAL…"));
    search.set_width_request(240);
    search.set_valign(gtk::Align::Center);
    search.set_tooltip_text(Some("Search the TIDAL catalog · Ctrl+K"));
    header.append(&search);
    main.append(&header);
    let toolbar = hbox(10);
    toolbar.add_css_class("toolbar");
    let play_all = gtk::Button::with_label("▶  Play this page");
    play_all.add_css_class("primary");
    play_all.set_visible(false);
    toolbar.append(&play_all);
    let search_albums = gtk::ToggleButton::with_label("Search albums");
    search_albums.add_css_class("album-search");
    search_albums.set_tooltip_text(Some(
        "Find albums in the TIDAL catalog using the search field",
    ));
    toolbar.append(&search_albums);
    let hint = label("Browse your collection", "muted");
    hint.add_css_class("small");
    hint.set_hexpand(true);
    toolbar.append(&hint);
    let quality_select = gtk::DropDown::from_strings(&["High quality", "Lossless"]);
    quality_select.set_tooltip_text(Some(
        "Requested streaming quality; availability depends on your session",
    ));
    toolbar.append(&quality_select);
    main.append(&toolbar);
    let discovery_filters = hbox(6);
    discovery_filters.add_css_class("discovery-filters");
    let filter_scroll = gtk::ScrolledWindow::new();
    filter_scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Never);
    filter_scroll.set_child(Some(&discovery_filters));
    main.append(&filter_scroll);
    let error_text = label("", "error-banner");
    error_text.set_wrap(true);
    error_text.set_hexpand(true);
    let error_row = hbox(0);
    error_row.append(&error_text);
    let dismiss = icon_button("window-close-symbolic", "Dismiss message");
    error_row.append(&dismiss);
    let error = gtk::Revealer::new();
    error.set_child(Some(&error_row));
    main.append(&error);
    let stack = gtk::Stack::new();
    stack.set_vexpand(true);
    main.append(&stack);
    let loading = vbox(16);
    loading.set_valign(gtk::Align::Center);
    loading.set_halign(gtk::Align::Center);
    let spinner = gtk::Spinner::new();
    spinner.set_size_request(28, 28);
    spinner.start();
    loading.append(&spinner);
    loading.append(&label("Getting your music ready…", "muted"));
    stack.add_named(&loading, Some("loading"));
    let login = vbox(18);
    login.set_valign(gtk::Align::Center);
    login.set_halign(gtk::Align::Center);
    login.set_width_request(360);
    login.append(&label("Your music. At home.", "empty-title"));
    let login_detail = label(
        "Connect your TIDAL account to explore your library.",
        "muted",
    );
    login_detail.set_wrap(true);
    login_detail.set_max_width_chars(48);
    login.append(&login_detail);
    let login_button = gtk::Button::with_label("Sign in with TIDAL");
    login_button.add_css_class("primary");
    login_button.set_halign(gtk::Align::Start);
    login.append(&login_button);
    stack.add_named(&login, Some("login"));
    let empty = vbox(12);
    empty.set_valign(gtk::Align::Center);
    empty.set_halign(gtk::Align::Center);
    empty.append(&label("Nothing here yet", "empty-title"));
    empty.append(&label(
        "Try another library section or search for a track.",
        "muted",
    ));
    stack.add_named(&empty, Some("empty"));
    let content = gtk::Stack::new();
    stack.add_named(&content, Some("content"));
    let grid = gtk::FlowBox::new();
    grid.set_selection_mode(gtk::SelectionMode::None);
    grid.set_min_children_per_line(1);
    grid.set_max_children_per_line(8);
    grid.set_row_spacing(18);
    grid.set_column_spacing(12);
    grid.set_homogeneous(true);
    grid.set_valign(gtk::Align::Start);
    grid.set_activate_on_single_click(true);
    grid.add_css_class("content");
    let grid_scroll = gtk::ScrolledWindow::new();
    grid_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    grid_scroll.set_child(Some(&grid));
    content.add_named(&grid_scroll, Some("grid"));
    let tracks = gtk::ListBox::new();
    tracks.set_activate_on_single_click(true);
    tracks.set_selection_mode(gtk::SelectionMode::Single);
    tracks.add_css_class("content");
    let track_scroll = gtk::ScrolledWindow::new();
    track_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    track_scroll.set_child(Some(&tracks));
    let listening = hbox(0);
    let hero = vbox(16);
    hero.add_css_class("album-hero");
    hero.set_width_request(220);
    let hero_art = vbox(0);
    hero.append(&hero_art);
    let hero_title = label("", "album-heading");
    hero_title.set_wrap(true);
    hero_title.set_max_width_chars(20);
    let hero_artist = label("", "muted");
    hero_artist.set_wrap(true);
    hero_artist.set_max_width_chars(24);
    hero.append(&hero_title);
    hero.append(&hero_artist);
    let save_album = gtk::Button::with_label("Save album");
    save_album.add_css_class("primary");
    hero.append(&save_album);
    let mode = Rc::new(Cell::new(0usize));
    let visualizer = gtk::DrawingArea::new();
    visualizer.add_css_class("visualizer");
    visualizer.set_accessible_role(gtk::AccessibleRole::Img);
    visualizer.set_content_width(220);
    visualizer.set_content_height(150);
    visualizer.update_property(&[gtk::accessible::Property::Label("Vines audio visualizer")]);
    hero.append(&visualizer);
    let viz_controls = hbox(4);
    let viz_mode = label(visualizer::MODES[0], "eyebrow");
    viz_mode.set_hexpand(true);
    let next_viz = icon_button("go-next-symbolic", "Next visualizer");
    let expand_viz = icon_button("view-fullscreen-symbolic", "Expand visualizer");
    viz_controls.append(&viz_mode);
    viz_controls.append(&next_viz);
    viz_controls.append(&expand_viz);
    hero.append(&viz_controls);
    {
        let mode = mode.clone();
        let area = visualizer.clone();
        next_viz.connect_clicked(move |_| {
            mode.set((mode.get() + 1) % visualizer::MODES.len());
            let name = visualizer::MODES[mode.get()];
            viz_mode.set_text(name);
            area.update_property(&[gtk::accessible::Property::Label(&format!(
                "{name} audio visualizer"
            ))]);
            area.queue_draw();
        });
    }
    {
        let state = state.clone();
        let mode = mode.clone();
        let app = app.clone();
        expand_viz.connect_clicked(move |_| {
            let mode = Rc::new(Cell::new(mode.get()));
            let win = gtk::ApplicationWindow::builder()
                .application(&app)
                .title("TIDAL Visualizer")
                .default_width(1100)
                .default_height(700)
                .build();
            win.add_css_class("tidal-window");
            let body = vbox(8);
            let top = hbox(12);
            top.set_margin_start(16);
            top.set_margin_end(16);
            let title = label(visualizer::MODES[mode.get()], "page-title");
            title.set_hexpand(true);
            top.append(&title);
            let cycle = gtk::Button::with_label("Next visualizer");
            top.append(&cycle);
            let close = gtk::Button::with_label("Close visualizer");
            top.append(&close);
            body.append(&top);
            let area = gtk::DrawingArea::new();
            area.add_css_class("visualizer");
            area.set_vexpand(true);
            body.append(&area);
            visualizer::attach(&area, state.clone(), mode.clone());
            win.set_child(Some(&body));
            {
                let mode = mode.clone();
                cycle.connect_clicked(move |_| {
                    mode.set((mode.get() + 1) % visualizer::MODES.len());
                    title.set_text(visualizer::MODES[mode.get()]);
                });
            }
            {
                let win = win.clone();
                close.connect_clicked(move |_| win.close());
            }
            let keys = gtk::EventControllerKey::new();
            {
                let win = win.clone();
                keys.connect_key_pressed(move |_, key, _, _| {
                    if key == gdk::Key::Escape {
                        win.close();
                        return glib::Propagation::Stop;
                    }
                    glib::Propagation::Proceed
                });
            }
            win.add_controller(keys);
            win.present();
            win.fullscreen();
        });
    }
    let hero_scroll = gtk::ScrolledWindow::new();
    hero_scroll.set_hexpand(false);
    hero_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    hero_scroll.set_child(Some(&hero));
    listening.append(&hero_scroll);
    track_scroll.set_hexpand(true);
    listening.append(&track_scroll);
    // Hide the scroll wrapper with its hero, otherwise an empty column remains.
    hero.bind_property("visible", &hero_scroll, "visible")
        .sync_create()
        .build();
    content.add_named(&listening, Some("tracks"));
    let pagination = hbox(12);
    pagination.add_css_class("pagination");
    let page_label = label("", "muted");
    page_label.add_css_class("small");
    page_label.set_hexpand(true);
    pagination.append(&page_label);
    let previous = icon_button("go-previous-symbolic", "Previous page");
    let next = icon_button("go-next-symbolic", "Next page");
    previous.set_sensitive(false);
    next.set_sensitive(false);
    pagination.append(&previous);
    pagination.append(&next);
    main.append(&pagination);
    let player = hbox(24);
    player.add_css_class("now-playing");
    root.append(&player);
    let now_art = hbox(0);
    let (placeholder, _) = cover(None, 48, &state);
    now_art.append(&placeholder);
    player.append(&now_art);
    let names = vbox(5);
    names.set_width_request(190);
    let now_title = label("Ready when you are", "track-title");
    now_title.set_ellipsize(pango::EllipsizeMode::End);
    now_title.set_max_width_chars(26);
    let now_artist = label("Choose something you love", "muted");
    now_artist.add_css_class("small");
    now_artist.set_ellipsize(pango::EllipsizeMode::End);
    now_artist.set_max_width_chars(26);
    names.append(&now_title);
    names.append(&now_artist);
    player.append(&names);
    let controls = hbox(8);
    let prev_track = icon_button("media-skip-backward-symbolic", "Previous track");
    prev_track.set_sensitive(false);
    let play = icon_button("media-playback-start-symbolic", "Play / pause · Space");
    play.add_css_class("play-toggle");
    play.set_halign(gtk::Align::Center);
    play.set_valign(gtk::Align::Center);
    play.set_size_request(40, 40);
    let next_track = icon_button("media-skip-forward-symbolic", "Next track");
    next_track.set_sensitive(false);
    controls.append(&prev_track);
    controls.append(&play);
    controls.append(&next_track);
    player.append(&controls);
    let progress = vbox(1);
    progress.set_hexpand(true);
    let seek = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 1.0);
    seek.set_draw_value(false);
    seek.set_sensitive(false);
    seek.update_property(&[gtk::accessible::Property::Label("Playback position")]);
    progress.append(&seek);
    let timestamps = hbox(8);
    let elapsed = label("0:00", "muted");
    elapsed.add_css_class("small");
    elapsed.set_hexpand(true);
    let duration = label("0:00", "muted");
    duration.add_css_class("small");
    timestamps.append(&elapsed);
    timestamps.append(&duration);
    progress.append(&timestamps);
    player.append(&progress);
    let quality = label("HIGH", "theme-chip");
    quality.set_valign(gtk::Align::Center);
    player.append(&quality);
    let volume = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
    volume.set_draw_value(false);
    volume.set_width_request(85);
    volume.set_value(35.0);
    volume.set_tooltip_text(Some("Volume"));
    volume.update_property(&[gtk::accessible::Property::Label("Volume")]);
    let speaker = gtk::Image::from_icon_name("audio-volume-medium-symbolic");
    player.append(&speaker);
    player.append(&volume);
    let ui = Ui {
        window: window.clone(),
        title,
        toolbar: toolbar.clone(),
        discovery_filters,
        hero,
        hero_art,
        hero_title,
        hero_artist,
        save_album,
        visualizer,
        subtitle,
        theme: theme_label,
        stack,
        content,
        grid,
        tracks,
        error,
        error_text,
        previous,
        next,
        page: page_label,
        pagination: pagination.clone(),
        back,
        nav,
        account,
        login_button,
        login_detail,
        sign_in: reconnect.clone(),
        sign_out: logout.clone(),
        play,
        previous_track: prev_track.clone(),
        next_track: next_track.clone(),
        now_title,
        now_artist,
        now_art,
        seek,
        elapsed,
        duration,
        quality,
        search,
        album_search: search_albums.clone(),
    };
    let updating = Rc::new(Cell::new(false));
    visualizer::attach(&ui.visualizer, state.clone(), mode);
    {
        let ui = ui.clone();
        let state = state.clone();
        let tx = tx.clone();
        search_albums.connect_toggled(move |button| {
            let text = ui.search.text().trim().to_owned();
            if text.is_empty() {
                ui.search.grab_focus();
                return;
            }
            navigate(
                &ui,
                &state,
                &tx,
                if button.is_active() {
                    View::SearchAlbums(text)
                } else {
                    View::Search(text)
                },
                1,
                true,
            );
        });
    }
    {
        let state = state.clone();
        let tx = tx.clone();
        ui.save_album.connect_clicked(move |button| {
            if let View::Album { id, .. } = state.borrow().view {
                button.set_sensitive(false);
                send(&tx, Action::SaveAlbum { id });
            }
        });
    }

    // Desktop actions use the same handlers as clicks and keyboard shortcuts.
    // They also make the application usable from Omarchy launcher hooks.
    let browse_action = gio::SimpleAction::new("browse", Some(glib::VariantTy::STRING));
    {
        let ui = ui.clone();
        let state = state.clone();
        let tx = tx.clone();
        browse_action.connect_activate(move |_, param| {
            let Some(value) = param.and_then(|p| p.str()) else {
                return;
            };
            let view = match value {
                "explore" => View::Explore,
                "albums" => View::Albums,
                "favorites" => View::Favorites,
                "playlists" => View::Playlists,
                value if value.starts_with("search:") => View::Search(value[7..].to_owned()),
                _ => return,
            };
            navigate(&ui, &state, &tx, view, 1, true);
        });
    }
    app.add_action(&browse_action);
    let open_action = gio::SimpleAction::new("open-item", Some(glib::VariantTy::UINT32));
    {
        let ui = ui.clone();
        let state = state.clone();
        let tx = tx.clone();
        open_action.connect_activate(move |_, param| {
            if let Some(index) = param.and_then(|p| p.get::<u32>()) {
                activate(&ui, &state, &tx, index as usize);
            }
        });
    }
    app.add_action(&open_action);
    for (name, delta) in [("next-page", 1), ("previous-page", -1)] {
        let action = gio::SimpleAction::new(name, None);
        let ui = ui.clone();
        let state = state.clone();
        let tx = tx.clone();
        action.connect_activate(move |_, _| {
            let (view, page) = {
                let s = state.borrow();
                (s.view.clone(), (s.page as i64 + delta).max(1) as u32)
            };
            navigate(&ui, &state, &tx, view, page, false);
        });
        app.add_action(&action);
    }
    let toggle_action = gio::SimpleAction::new("toggle-playback", None);
    {
        let tx = tx.clone();
        toggle_action.connect_activate(move |_, _| send(&tx, Action::Toggle));
    }
    app.add_action(&toggle_action);
    let quit_action = gio::SimpleAction::new("quit", None);
    {
        let window = window.clone();
        quit_action.connect_activate(move |_, _| window.close());
    }
    app.add_action(&quit_action);

    for (index, button) in ui.nav.iter().enumerate() {
        let ui = ui.clone();
        let state = state.clone();
        let tx = tx.clone();
        button.connect_clicked(move |_| {
            let view = match index {
                0 => View::Albums,
                1 => View::Favorites,
                2 => View::Playlists,
                _ => View::Explore,
            };
            navigate(&ui, &state, &tx, view, 1, true);
        });
    }
    {
        let ui = ui.clone();
        let state = state.clone();
        let tx = tx.clone();
        ui.tracks.clone().connect_row_activated(move |_, row| {
            activate(&ui, &state, &tx, row.index() as usize);
        });
    }
    {
        let tx = tx.clone();
        let state = state.clone();
        play_all.connect_clicked(move |_| {
            send(
                &tx,
                Action::Play {
                    index: 0,
                    epoch: state.borrow().epoch,
                },
            )
        });
    }
    {
        let ui = ui.clone();
        let state = state.clone();
        let tx = tx.clone();
        ui.back.clone().connect_clicked(move |_| {
            let last = state.borrow_mut().history.pop();
            if let Some((view, page)) = last {
                navigate(&ui, &state, &tx, view, page, false);
            }
        });
    }
    for (button, delta) in [(ui.previous.clone(), -1), (ui.next.clone(), 1)] {
        let ui = ui.clone();
        let state = state.clone();
        let tx = tx.clone();
        button.connect_clicked(move |_| {
            let (view, page) = {
                let s = state.borrow();
                (s.view.clone(), (s.page as i64 + delta).max(1) as u32)
            };
            navigate(&ui, &state, &tx, view, page, false);
        });
    }
    {
        let ui = ui.clone();
        let state = state.clone();
        let tx = tx.clone();
        ui.search.clone().connect_activate(move |entry| {
            let text = entry.text().trim().to_owned();
            if !text.is_empty() {
                navigate(
                    &ui,
                    &state,
                    &tx,
                    if ui.album_search.is_active() {
                        View::SearchAlbums(text)
                    } else {
                        View::Search(text)
                    },
                    1,
                    true,
                );
            }
        });
    }
    for button in [ui.login_button.clone(), reconnect] {
        let tx = tx.clone();
        let ui = ui.clone();
        button.connect_clicked(move |_| {
            ui.login_button.set_sensitive(false);
            ui.login_detail
                .set_text("Opening TIDAL’s secure sign-in page…");
            ui.stack.set_visible_child_name("login");
            send(&tx, Action::Login);
        });
    }
    {
        let tx = tx.clone();
        logout.connect_clicked(move |_| send(&tx, Action::Logout));
    }
    {
        let tx = tx.clone();
        quality_select
            .connect_selected_notify(move |d| send(&tx, Action::Quality(d.selected() == 1)));
    }
    {
        let tx = tx.clone();
        let state = state.clone();
        ui.play.connect_clicked(move |_| {
            if state.borrow().last_playback.is_some() {
                send(&tx, Action::Toggle);
            } else if !state.borrow().items.is_empty()
                && matches!(state.borrow().items[0].key, Key::Track(_))
            {
                send(
                    &tx,
                    Action::Play {
                        index: 0,
                        epoch: state.borrow().epoch,
                    },
                );
            }
        });
    }
    {
        let tx = tx.clone();
        prev_track.connect_clicked(move |_| send(&tx, Action::Previous));
    }
    {
        let tx = tx.clone();
        next_track.connect_clicked(move |_| send(&tx, Action::Next));
    }
    {
        let tx = tx.clone();
        let updating = updating.clone();
        ui.seek.connect_value_changed(move |scale| {
            if !updating.get() {
                send(&tx, Action::Seek(scale.value()));
            }
        });
    }
    {
        let tx = tx.clone();
        volume.connect_value_changed(move |v| send(&tx, Action::Volume(v.value())));
    }
    {
        let error = ui.error.clone();
        dismiss.connect_clicked(move |_| error.set_reveal_child(false));
    }
    {
        let tx = tx.clone();
        window.connect_close_request(move |_| {
            send(&tx, Action::Shutdown);
            glib::Propagation::Proceed
        });
    }
    let key = gtk::EventControllerKey::new();
    {
        let ui = ui.clone();
        let state = state.clone();
        let tx = tx.clone();
        key.connect_key_pressed(move |_, key, _, mods| {
            if mods.contains(gdk::ModifierType::CONTROL_MASK) && key == gdk::Key::k {
                ui.search.grab_focus();
                return glib::Propagation::Stop;
            }
            if mods.contains(gdk::ModifierType::CONTROL_MASK) && key == gdk::Key::q {
                ui.window.close();
                return glib::Propagation::Stop;
            }
            let editing = gtk::prelude::GtkWindowExt::focus(&ui.window).is_some_and(|f| {
                f.is::<gtk::Text>() || f.is::<gtk::SearchEntry>() || f.is::<gtk::Entry>()
            });
            if key == gdk::Key::space && !editing {
                if state.borrow().last_playback.is_some() {
                    send(&tx, Action::Toggle);
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
    }
    window.add_controller(key);

    {
        let ui = ui.clone();
        let state = state.clone();
        let tx = tx.clone();
        let updating = updating.clone();
        glib::timeout_add_local(Duration::from_millis(40), move || {
            if !ui.window.is_visible() {
                return glib::ControlFlow::Break;
            }
            while let Ok(event) = events.try_recv() {
                match event {
                    Event::Level(level) => {
                        if let Some(playback) = state.borrow_mut().last_playback.as_mut() {
                            playback.level = level;
                        }
                    }
                    Event::AlbumSaved(id) => {
                        if matches!(state.borrow().view,View::Album{id:current,..} if current==id) {
                            ui.save_album.set_label("Saved to your albums");
                            ui.save_album.set_sensitive(false);
                        }
                    }
                    Event::Ready => {
                        ui.toolbar.set_visible(true);
                        ui.search.set_sensitive(true);
                        for button in &ui.nav {
                            button.set_sensitive(true);
                        }
                        ui.sign_in.set_visible(false);
                        ui.sign_out.set_sensitive(true);
                        ui.account.set_text("●  Account connected");
                        ui.login_button.set_sensitive(true);
                        navigate(&ui, &state, &tx, View::Albums, 1, false);
                    }
                    Event::NeedsLogin => {
                        gtk::prelude::GtkWindowExt::set_focus(&ui.window, None::<&gtk::Widget>);
                        ui.title.set_text("Welcome back");
                        ui.subtitle.set_text("Sign in to access your library");
                        ui.title
                            .update_state(&[gtk::accessible::State::Busy(false)]);
                        ui.toolbar.set_visible(false);
                        ui.discovery_filters.set_visible(false);
                        ui.search.set_text("");
                        ui.search.set_sensitive(false);
                        ui.back.set_sensitive(false);
                        for button in &ui.nav {
                            button.set_sensitive(false);
                            button.remove_css_class("active");
                        }
                        {
                            let mut s = state.borrow_mut();
                            s.history.clear();
                            s.items.clear();
                            s.covers.clear();
                        }
                        while let Some(child) = ui.grid.first_child() {
                            ui.grid.remove(&child);
                        }
                        while let Some(child) = ui.tracks.first_child() {
                            ui.tracks.remove(&child);
                        }
                        ui.sign_in.set_visible(true);
                        ui.sign_out.set_sensitive(false);
                        ui.previous_track.set_sensitive(false);
                        ui.next_track.set_sensitive(false);
                        ui.seek.set_sensitive(false);
                        ui.play.set_icon_name("media-playback-start-symbolic");
                        ui.pagination.set_visible(false);
                        ui.error.set_reveal_child(false);
                        state.borrow_mut().now_cover = None;
                        while let Some(child) = ui.now_art.first_child() {
                            ui.now_art.remove(&child);
                        }
                        let (art, _) = cover(None, 48, &state);
                        ui.now_art.append(&art);
                        updating.set(true);
                        ui.seek.set_range(0.0, 1.0);
                        ui.seek.set_value(0.0);
                        updating.set(false);
                        ui.elapsed.set_text("0:00");
                        ui.duration.set_text("0:00");
                        ui.account.set_text("Not connected");
                        ui.login_button.set_sensitive(true);
                        ui.login_detail
                            .set_text("Connect your TIDAL account to explore your library.");
                        ui.stack.set_visible_child_name("login");
                        ui.login_button.grab_focus();
                        state.borrow_mut().last_playback = None;
                        ui.now_title.set_text("Ready when you are");
                        ui.now_artist.set_text("Choose something you love");
                    }
                    Event::LoginCode(url, code) => {
                        ui.login_detail.set_text(&format!(
                            "Authorize in your browser.\n\n{url}\n\nDevice code: {code}"
                        ));
                    }
                    Event::Page(page) => {
                        if page.epoch != state.borrow().epoch {
                            continue;
                        }
                        play_all.set_label(match page.view {
                            View::Album { .. } => "Play this album",
                            View::Playlist { .. } => "Play this playlist",
                            _ => "Play these tracks",
                        });
                        play_all.set_visible(!page.view.is_grid() && !page.items.is_empty());
                        display(&ui, &state, &tx, page);
                    }
                    Event::Artwork(key, path) => {
                        let mut s = state.borrow_mut();
                        if let Some(covers) = s.covers.get(&key) {
                            for cover in covers {
                                set_cover(cover, &path);
                            }
                        }
                        if let Some((id, cover)) = &s.now_cover {
                            if id == &key {
                                set_cover(cover, &path);
                            }
                        }
                        s.art.insert(key, path);
                    }
                    Event::Playback(value) => playback(&ui, &state, value, &updating),
                    Event::LoadingTrack(title) => {
                        ui.now_title.set_text(&title);
                        ui.now_artist.set_text("Connecting to stream…");
                        ui.error.set_reveal_child(false);
                    }
                    Event::Error(message) => {
                        ui.save_album.set_sensitive(true);
                        ui.title
                            .update_state(&[gtk::accessible::State::Busy(false)]);
                        if ui.now_artist.text() == "Connecting to stream…" {
                            ui.now_artist.set_text("Playback unavailable");
                            state.borrow_mut().last_playback = None;
                            ui.previous_track.set_sensitive(false);
                            ui.next_track.set_sensitive(false);
                            ui.seek.set_sensitive(false);
                        }
                        ui.error_text.set_text(&message);
                        ui.error.set_reveal_child(true);
                        ui.login_button.set_sensitive(true);
                        if ui.stack.visible_child_name().as_deref() == Some("loading") {
                            ui.stack.set_visible_child_name("empty");
                        }
                    }
                }
            }
            glib::ControlFlow::Continue
        });
    }
    {
        let ui = ui.clone();
        let state = state.clone();
        glib::timeout_add_local(Duration::from_secs(2), move || {
            if !ui.window.is_visible() {
                return glib::ControlFlow::Break;
            }
            let theme = Theme::load();
            let css = theme.css();
            if css != state.borrow().theme_css {
                provider.load_from_string(&css);
                ui.theme.set_text(&format!("●  {}", theme.name));
                state.borrow_mut().theme_css = css;
            }
            glib::ControlFlow::Continue
        });
    }
    window.present();
}
