use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, Frame, Rounding, Slider};
use notify_rust::Notification;
use rusqlite::Connection;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, MenuId};
use tray_icon::TrayIconBuilder;

use crate::db;
use crate::download;
use crate::feed;
use crate::opml;
use crate::playback::Player;
use crate::sync;
use crate::types::*;

fn data_dir() -> PathBuf {
    let xdg = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            PathBuf::from(home).join(".local/share")
        });
    xdg.join("russtly")
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn strip_html(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut in_tag = false;
    let mut in_entity = false;
    let mut entity_buf = String::new();
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            '&' if !in_tag => {
                in_entity = true;
                entity_buf.clear();
            }
            ';' if in_entity => {
                in_entity = false;
                let decoded = match entity_buf.as_str() {
                    "amp" => "&",
                    "lt" => "<",
                    "gt" => ">",
                    "quot" => "\"",
                    "apos" => "'",
                    "nbsp" => " ",
                    _ => "",
                };
                result.push_str(decoded);
            }
            _ if in_entity => entity_buf.push(c),
            _ if !in_tag && !in_entity => result.push(c),
            _ => {}
        }
    }
    result
}

fn format_duration(secs: Option<i64>) -> String {
    match secs {
        Some(s) if s < 0 => "??:??".to_string(),
        Some(s) => {
            let h = s / 3600;
            let m = (s % 3600) / 60;
            let sec = s % 60;
            if h > 0 {
                format!("{:02}:{:02}:{:02}", h, m, sec)
            } else {
                format!("{:02}:{:02}", m, sec)
            }
        }
        None => "??:??".to_string(),
    }
}

fn format_duration_dur(d: Duration) -> String {
    let total = d.as_secs();
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{:02}:{:02}:{:02}", h, m, s)
    } else {
        format!("{:02}:{:02}", m, s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum EpisodeFilter {
    All,
    Unplayed,
    Downloaded,
}

fn format_date(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc2822(s) {
        dt.format("%b %d, %Y").to_string()
    } else {
        s.split(' ')
            .take(4)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == ' ' || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim()
        .to_string()
}

pub struct RuSStlyApp {
    conn: Connection,
    feeds: Vec<Feed>,
    selected_feed_id: Option<i64>,
    episodes: Vec<Episode>,
    add_feed_url: String,
    add_feed_error: String,
    tx: mpsc::Sender<AppMessage>,
    rx: mpsc::Receiver<AppMessage>,
    player: Player,
    volume: f32,
    download_progress: HashMap<i64, f64>,
    sync_targets: Vec<SyncTarget>,
    new_sync_label: String,
    new_sync_path: String,
    sync_status: String,
    client: reqwest::Client,
    rt_handle: tokio::runtime::Handle,
    expanded_episodes: HashSet<i64>,
    last_position_save: std::time::Instant,
    pending_sync_episodes: HashSet<i64>,

    // Tray
    _tray: Option<tray_icon::TrayIcon>,
    tray_show_id: MenuId,
    tray_quit_id: MenuId,
    tray_initialized: bool,
    should_hide: bool,
    should_quit: bool,

    // Settings
    show_settings: bool,
    download_dir: String,
    auto_download: bool,
    auto_mark_played: bool,
    auto_mark_threshold: f32,
    playback_speed: f32,
    opml_import_path: String,
    opml_export_path: String,
    opml_status: String,

    // New features
    feed_unplayed_counts: HashMap<i64, usize>,
    episode_filter: EpisodeFilter,
    dark_mode: bool,
    sleep_timer_minutes: i32,
    sleep_timer_started_at: Option<Instant>,

    // Concurrency control
    download_semaphore: std::sync::Arc<tokio::sync::Semaphore>,
    max_episodes: usize,
}

impl RuSStlyApp {
    pub fn new() -> Self {
        let rt_handle = tokio::runtime::Handle::current();
        let data_path = data_dir();
        std::fs::create_dir_all(&data_path).ok();
        let db_path = data_path.join("russtly.db");
        let conn = Connection::open(&db_path).expect("Failed to open database");
        db::init_db(&conn).expect("Failed to initialize database");

        let feeds = db::get_feeds(&conn).unwrap_or_default();
        let selected_feed_id = feeds.first().map(|f| f.id);
        let episodes = selected_feed_id
            .and_then(|id| db::get_episodes(&conn, id).ok())
            .unwrap_or_default();

        let _ = db::migrate_sync_target(&conn);
        let sync_targets = db::get_sync_targets(&conn).unwrap_or_default();
        let volume = db::get_setting(&conn, "volume")
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(1.0);
        let download_dir = db::get_setting(&conn, "download_dir").unwrap_or_else(|| {
            home_dir().join("Podcasts").to_string_lossy().to_string()
        });
        let auto_download = db::get_setting(&conn, "auto_download")
            .map(|v| v == "true")
            .unwrap_or(false);
        let auto_mark_played = db::get_setting(&conn, "auto_mark_played")
            .map(|v| v == "true")
            .unwrap_or(true);
        let auto_mark_threshold = db::get_setting(&conn, "auto_mark_threshold")
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(0.95);
        let playback_speed = db::get_setting(&conn, "playback_speed")
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(1.0);
        let dark_mode = db::get_setting(&conn, "dark_mode")
            .map(|v| v == "true")
            .unwrap_or(false);
        let max_episodes = db::get_setting(&conn, "max_episodes")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(200);

        let (tx, rx) = mpsc::channel();

        let show_id: MenuId = "show".into();
        let quit_id: MenuId = "quit".into();

        let mut app = RuSStlyApp {
            conn,
            feeds,
            selected_feed_id,
            episodes,
            add_feed_url: String::new(),
            add_feed_error: String::new(),
            tx,
            rx,
            player: Player::new(),
            volume,
            download_progress: HashMap::new(),
            sync_targets,
            new_sync_label: String::new(),
            new_sync_path: String::new(),
            sync_status: String::new(),
            client: reqwest::Client::builder()
                .user_agent("Mozilla/5.0 (compatible; RuSStly/0.1; +https://github.com/goldfang/russtly)")
                .connect_timeout(Duration::from_secs(30))
                .timeout(Duration::from_secs(180))
                .redirect(reqwest::redirect::Policy::default())
                .build()
                .expect("Failed to create HTTP client"),
            rt_handle,
            expanded_episodes: HashSet::new(),
            last_position_save: std::time::Instant::now(),
            pending_sync_episodes: HashSet::new(),
            _tray: None,
            tray_show_id: show_id,
            tray_quit_id: quit_id,
            tray_initialized: false,
            should_hide: false,
            should_quit: false,

            show_settings: false,
            download_dir,
            auto_download,
            auto_mark_played,
            auto_mark_threshold,
            playback_speed,
            opml_import_path: String::new(),
            opml_export_path: String::new(),
            opml_status: String::new(),

            feed_unplayed_counts: HashMap::new(),
            episode_filter: EpisodeFilter::All,
            dark_mode,
            sleep_timer_minutes: 0,
            sleep_timer_started_at: None,

            download_semaphore: std::sync::Arc::new(tokio::sync::Semaphore::new(4)),
            max_episodes,
        };

        app.compute_unplayed_counts();
        app
    }

    fn set_volume(&mut self, vol: f32) {
        self.volume = vol.clamp(0.0, 1.0);
        self.player.set_volume(self.volume);
        let _ = db::set_setting(&self.conn, "volume", &self.volume.to_string());
    }

    fn compute_unplayed_counts(&mut self) {
        self.feed_unplayed_counts.clear();
        for feed in &self.feeds {
            if let Ok(eps) = db::get_episodes(&self.conn, feed.id) {
                let count = eps.iter().filter(|e| !e.played).count();
                self.feed_unplayed_counts.insert(feed.id, count);
            }
        }
    }

    fn load_episodes(&mut self) {
        if let Some(feed_id) = self.selected_feed_id {
            self.episodes = db::get_episodes(&self.conn, feed_id).unwrap_or_default();
            self.episodes.truncate(self.max_episodes);
        } else {
            self.episodes.clear();
        }
        self.compute_unplayed_counts();
    }

    fn find_episode_title(&self, episode_id: i64) -> String {
        for feed in &self.feeds {
            if let Ok(eps) = db::get_episodes(&self.conn, feed.id) {
                if let Some(ep) = eps.iter().find(|e| e.id == episode_id) {
                    return ep.title.clone();
                }
            }
        }
        String::new()
    }

    fn init_tray(&mut self) {
        // tray-icon uses libappindicator (X11 only; Wayland needs StatusNotifier)
        if std::env::var("XDG_SESSION_TYPE").as_deref() == Ok("wayland") || gtk::init().is_err() {
            return;
        }
        let show = MenuItem::with_id(self.tray_show_id.clone(), "Show", true, None::<_>);
        let quit = MenuItem::with_id(self.tray_quit_id.clone(), "Quit", true, None::<_>);
        if let Some(menu) = Menu::with_items(&[&show, &quit]).ok() {
            self._tray = TrayIconBuilder::new()
                .with_menu(Box::new(menu))
                .with_tooltip("RuSStly")
                .build()
                .ok();
        }
    }

    fn process_messages(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                AppMessage::FeedFetched {
                    feed_id,
                    title,
                    description,
                    image_url,
                    episodes,
                } => {
                    let _ = db::update_feed(&self.conn, feed_id, &title, &description, &image_url);
                    let episodes: Vec<_> = episodes.into_iter().take(self.max_episodes).collect();
                    let results = db::upsert_episodes_batch(&mut self.conn, feed_id, &episodes)
                        .unwrap_or_default();
                    let new_ep_count = results.iter().filter(|(_, was_down)| !was_down).count();
                    if self.auto_download {
                        let show_title = self
                            .feeds
                            .iter()
                            .find(|f| f.id == feed_id)
                            .map(|f| f.title.clone())
                            .unwrap_or_default();
                        let dest_dir = PathBuf::from(&self.download_dir)
                            .join(sanitize_name(&show_title));
                        for ((ep_id, was_downloaded), ep) in results.iter().zip(episodes.iter()) {
                            if !was_downloaded {
                                let dest_path = dest_dir.join(format!(
                                    "{}.mp3",
                                    sanitize_name(&ep.title)
                                ));
                                let tx = self.tx.clone();
                                let client = self.client.clone();
                                let url = ep.audio_url.clone();
                                let id = *ep_id;
                                let sem = self.download_semaphore.clone();
                                self.rt_handle.spawn(async move {
                                    let _permit = sem.acquire().await;
                                    download::download_episode(
                                        &client, &url, &dest_path, id, tx,
                                    )
                                    .await;
                                });
                            }
                        }
                    }
                    self.feeds = db::get_feeds(&self.conn).unwrap_or_default();
                    self.load_episodes();
                    if self.auto_download && new_ep_count > 0 {
                        self.add_feed_error = format!(
                            "Feed updated: {} episodes ({} new, auto-downloading)",
                            episodes.len(),
                            new_ep_count
                        );
                    } else {
                        self.add_feed_error = format!("Feed updated: {} episodes", episodes.len());
                    }
                }
                AppMessage::FeedFetchFailed { url, error } => {
                    self.add_feed_error = format!("Failed to fetch {}: {}", url, error);
                }
                AppMessage::DownloadProgress {
                    episode_id,
                    progress,
                } => {
                    self.download_progress.insert(episode_id, progress);
                }
                AppMessage::DownloadComplete {
                    episode_id,
                    path,
                } => {
                    self.download_progress.remove(&episode_id);
                    let _ = db::set_episode_downloaded(&self.conn, episode_id, &path);
                    self.load_episodes();
                    let title = self.find_episode_title(episode_id);
                    let _ = send_notification("Download Complete", &title);
                }
                AppMessage::DownloadFailed { episode_id, error } => {
                    self.download_progress.remove(&episode_id);
                    self.add_feed_error = format!("Download failed: {}", error);
                    let title = self.find_episode_title(episode_id);
                    let _ = send_notification("Download Failed", &format!("{}: {}", title, error));
                }
                AppMessage::SyncResult {
                    episode_id,
                    success,
                    message,
                } => {
                    self.pending_sync_episodes.remove(&episode_id);
                    self.sync_status = if success {
                        format!("Synced: {}", message)
                    } else {
                        format!("Sync failed: {}", message)
                    };
                }
            }
        }
    }

    fn update_playback(&mut self) {
        if let Some(episode_id) = self.player.current_episode_id() {
            let pos = self.player.current_position().as_secs_f64();
            let is_empty = self.player.is_empty() && !self.player.is_paused();

            // Auto-mark as played when near end
            if !is_empty && self.auto_mark_played {
                if let Some(total) = self.player.total_duration() {
                    let total_secs = total.as_secs_f64();
                    if total_secs > 0.0 {
                        let progress = pos / total_secs;
                        if progress >= self.auto_mark_threshold as f64 {
                            let _ = db::set_episode_played(&self.conn, episode_id);
                            self.load_episodes();
                        }
                    }
                }
            }

            if is_empty {
                let _ = db::set_episode_played(&self.conn, episode_id);
                let _ = db::update_episode_state(&self.conn, episode_id, true, pos);
                self.player.stop();
                self.load_episodes();
            } else if self.last_position_save.elapsed() >= std::time::Duration::from_secs(2) {
                let _ = db::update_episode_state(&self.conn, episode_id, false, pos);
                self.last_position_save = std::time::Instant::now();
            }
        }
    }
}

impl eframe::App for RuSStlyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.process_messages();
        self.update_playback();
        ctx.request_repaint_after(std::time::Duration::from_millis(250));

        // Theme
        if self.dark_mode {
            ctx.set_visuals(egui::Visuals::dark());
        } else {
            ctx.set_visuals(egui::Visuals::light());
        }

        // Sleep timer check
        if self.sleep_timer_minutes > 0 {
            if let Some(start) = self.sleep_timer_started_at {
                if start.elapsed().as_secs_f64() >= self.sleep_timer_minutes as f64 * 60.0 {
                    self.player.pause();
                    self.sleep_timer_minutes = 0;
                    self.sleep_timer_started_at = None;
                }
            }
        }

        // Initialize tray icon on first frame (needs GTK initialized)
        if !self.tray_initialized {
            self.init_tray();
            self.tray_initialized = true;
        }

        // Keyboard shortcuts
        {
            let mut toggle_play = false;
            let mut skip_back = false;
            let mut skip_fwd = false;
            let mut vol_up = false;
            let mut vol_down = false;

            ctx.input(|i| {
                toggle_play = i.key_pressed(egui::Key::Space);
                skip_back = i.key_pressed(egui::Key::ArrowLeft);
                skip_fwd = i.key_pressed(egui::Key::ArrowRight);
                vol_up = i.key_pressed(egui::Key::Equals);
                vol_down = i.key_pressed(egui::Key::Minus);
            });

            if toggle_play {
                self.player.toggle_play_pause();
            }
            if skip_back {
                self.player.skip_backward(15.0);
            }
            if skip_fwd {
                self.player.skip_forward(30.0);
            }
            if vol_up {
                self.set_volume(self.volume + 0.1);
            }
            if vol_down {
                self.set_volume(self.volume - 0.1);
            }
        }

        // --- Sidebar ---
        egui::SidePanel::left("sidebar")
            .resizable(true)
            .default_width(260.0)
            .min_width(180.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("Subscriptions");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("⚙").clicked() {
                            self.show_settings = !self.show_settings;
                        }
                    });
                });
                ui.separator();

                ui.label("URL:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.add_feed_url)
                        .hint_text("https://example.com/feed.xml")
                        .desired_width(f32::INFINITY),
                );
                if ui.button("Add Feed").clicked() {
                    let url = self.add_feed_url.trim().to_string();
                    if url.is_empty() {
                        self.add_feed_error = "Enter a URL".to_string();
                    } else if self.feeds.iter().any(|f| f.url == url) {
                        self.add_feed_error = "Already subscribed".to_string();
                    } else {
                        self.add_feed_error = "Fetching...".to_string();
                        let tx = self.tx.clone();
                        let client = self.client.clone();
                        let url_clone = url.clone();
                        match db::add_feed(&self.conn, &url, "Fetching...", "", "") {
                            Ok(feed_id) => {
                                self.feeds = db::get_feeds(&self.conn).unwrap_or_default();
                                self.selected_feed_id = Some(feed_id);
                                self.load_episodes();
                                self.rt_handle.spawn(async move {
                                    match feed::fetch_feed(&client, &url_clone).await {
                                        Ok((title, description, image_url, episodes)) => {
                                            let _ = tx.send(AppMessage::FeedFetched {
                                                feed_id,
                                                title,
                                                description,
                                                image_url,
                                                episodes,
                                            });
                                        }
                                        Err(e) => {
                                            let _ = tx.send(AppMessage::FeedFetchFailed {
                                                url: url_clone,
                                                error: e,
                                            });
                                        }
                                    }
                                });
                            }
                            Err(e) => {
                                self.add_feed_error = format!("DB error: {}", e);
                            }
                        }
                    }
                }

                if !self.add_feed_error.is_empty() {
                    ui.colored_label(
                        if self.add_feed_error == "Fetching..." {
                            Color32::YELLOW
                        } else if self.add_feed_error.starts_with("Feed updated") {
                            Color32::GREEN
                        } else {
                            Color32::RED
                        },
                        &self.add_feed_error,
                    );
                }

                ui.separator();

                if ui.button("Refresh All").clicked() {
                    for feed in self.feeds.clone() {
                        let tx = self.tx.clone();
                        let client = self.client.clone();
                        self.rt_handle.spawn(async move {
                            if let Ok((title, description, image_url, episodes)) =
                                feed::fetch_feed(&client, &feed.url).await
                            {
                                let _ = tx.send(AppMessage::FeedFetched {
                                    feed_id: feed.id,
                                    title,
                                    description,
                                    image_url,
                                    episodes,
                                });
                            }
                        });
                    }
                }

                ui.separator();

                // Clone feed list to avoid borrow conflicts with closures
                let feed_list: Vec<Feed> = self.feeds.clone();

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let mut to_remove: Option<i64> = None;
                        let mut to_select: Option<i64> = None;
                        for feed in &feed_list {
                            let selected = self.selected_feed_id == Some(feed.id);
                            let bg_color = if selected {
                                ui.style().visuals.selection.bg_fill
                            } else {
                                Color32::TRANSPARENT
                            };

                            let unplayed = self.feed_unplayed_counts.get(&feed.id).copied().unwrap_or(0);

                            let mut text = egui::RichText::new(&feed.title).size(14.0);
                            if selected {
                                text = text.strong();
                            }
                            if unplayed > 0 {
                                text = text.color(Color32::WHITE);
                            }

                            let resp = Frame::none()
                                .fill(bg_color)
                                .rounding(Rounding::same(4.0))
                                .inner_margin(egui::Margin::symmetric(6.0, 3.0))
                                .show(ui, |ui| {
                                    ui.set_min_width(ui.available_width());
                                    ui.horizontal(|ui| {
                                        let label = egui::Label::new(text);
                                        let r = ui.add(label);
                                        if r.clicked() {
                                            to_select = Some(feed.id);
                                        }
                                        if unplayed > 0 {
                                            ui.label(
                                                egui::RichText::new(format!("({})", unplayed))
                                                    .size(11.0)
                                                    .color(Color32::YELLOW),
                                            );
                                        }
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                if ui.small_button("✕").clicked() {
                                                    to_remove = Some(feed.id);
                                                }
                                            },
                                        );
                                    });
                                });

                            if resp.response.clicked() {
                                to_select = Some(feed.id);
                            }
                        }

                        if let Some(id) = to_select {
                            self.selected_feed_id = Some(id);
                            self.load_episodes();
                        }

                        if let Some(id) = to_remove {
                            let _ = db::remove_feed(&self.conn, id);
                            self.feeds = db::get_feeds(&self.conn).unwrap_or_default();
                            if self.selected_feed_id == Some(id) {
                                self.selected_feed_id = self.feeds.first().map(|f| f.id);
                                self.load_episodes();
                            }
                        }
                    });
            });

        // --- Center Panel ---
        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(feed_id) = self.selected_feed_id {
                if let Some(feed) = self.feeds.iter().find(|f| f.id == feed_id) {
                    ui.heading(&feed.title);
                    if !feed.description.is_empty() {
                        ui.label(
                            egui::RichText::new(strip_html(&feed.description))
                                .size(12.0)
                                .color(Color32::GRAY),
                        );
                    }
                    ui.separator();
                }

                ui.horizontal(|ui| {
                    let count = self.sync_targets.len();
                    ui.label(format!("Sync targets: {}", count));
                });

                if !self.sync_status.is_empty() {
                    ui.colored_label(Color32::GREEN, &self.sync_status);
                }

                ui.separator();

                ui.horizontal(|ui| {
                    ui.label("Filter:");
                    ui.selectable_value(&mut self.episode_filter, EpisodeFilter::All, "All");
                    ui.selectable_value(
                        &mut self.episode_filter,
                        EpisodeFilter::Unplayed,
                        "Unplayed",
                    );
                    ui.selectable_value(
                        &mut self.episode_filter,
                        EpisodeFilter::Downloaded,
                        "Downloaded",
                    );
                });

                ui.separator();

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let filtered_episodes: Vec<Episode> = self
                            .episodes
                            .iter()
                            .filter(|e| match self.episode_filter {
                                EpisodeFilter::All => true,
                                EpisodeFilter::Unplayed => !e.played,
                                EpisodeFilter::Downloaded => e.downloaded,
                            })
                            .cloned()
                            .collect();
                        if filtered_episodes.is_empty() {
                            ui.label("No episodes match the filter.");
                            return;
                        }

                        for episode in &filtered_episodes {
                            let is_expanded = self.expanded_episodes.contains(&episode.id);
                            let downloaded = episode.downloaded;
                            let playing = self.player.current_episode_id() == Some(episode.id);
                            let is_downloading =
                                self.download_progress.contains_key(&episode.id);

                            let frame = Frame::group(ui.style())
                                .inner_margin(egui::Margin::symmetric(8.0, 6.0))
                                .rounding(Rounding::same(6.0));

                            frame.show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.vertical(|ui| {
                                        let title_color = if playing {
                                            Color32::from_rgb(0, 180, 255)
                                        } else if episode.played {
                                            Color32::GRAY
                                        } else {
                                            Color32::WHITE
                                        };
                                        ui.label(
                                            egui::RichText::new(&episode.title)
                                                .size(14.0)
                                                .color(title_color)
                                                .strong(),
                                        );

                                        ui.horizontal(|ui| {
                                            ui.spacing_mut().item_spacing.x = 8.0;
                                            let date_str = format_date(&episode.pub_date);
                                            if !date_str.is_empty() {
                                                ui.label(
                                                    egui::RichText::new(date_str)
                                                        .size(11.0)
                                                        .color(Color32::GRAY),
                                                );
                                            }
                                            ui.label(
                                                egui::RichText::new(format_duration(
                                                    episode.duration_secs,
                                                ))
                                                .size(11.0)
                                                .color(Color32::GRAY),
                                            );
                                            if episode.played {
                                                ui.label(
                                                    egui::RichText::new("✓ Played")
                                                        .size(11.0)
                                                        .color(Color32::GREEN),
                                                );
                                            }
                                            if downloaded {
                                                ui.label(
                                                    egui::RichText::new("✓ Downloaded")
                                                        .size(11.0)
                                                        .color(Color32::from_rgb(100, 200, 100)),
                                                );
                                            }
                                        });

                                        if let Some(progress) =
                                            self.download_progress.get(&episode.id)
                                        {
                                            ui.add(
                                                egui::ProgressBar::new(*progress as f32)
                                                    .desired_width(200.0)
                                                    .text(format!(
                                                        "{:.0}%",
                                                        *progress * 100.0
                                                    )),
                                            );
                                        }
                                    });

                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if downloaded
                                                && !self
                                                    .pending_sync_episodes
                                                    .contains(&episode.id)
                                                && !self.sync_targets.is_empty()
                                            {
                                                if ui.small_button("Sync").clicked() {
                                                    self.pending_sync_episodes
                                                        .insert(episode.id);
                                                    if let Some(ref dpath) =
                                                        episode.download_path
                                                    {
                                                        let feed_title = self
                                                            .feeds
                                                            .iter()
                                                            .find(|f| {
                                                                f.id == episode.feed_id
                                                            })
                                                            .map(|f| f.title.clone())
                                                            .unwrap_or_default();
                                                        let source = PathBuf::from(dpath);
                                                        let targets = self.sync_targets.clone();
                                                        let ep_title = episode.title.clone();
                                                        let tx = self.tx.clone();
                                                        let ep_id = episode.id;
                                                        self.rt_handle.spawn(async move {
                                                            let mut success_count = 0;
                                                            let mut first_err = String::new();
                                                            for target in &targets {
                                                                let target_path = PathBuf::from(&target.path);
                                                                match sync::sync_episode(
                                                                    &source,
                                                                    &target_path,
                                                                    &feed_title,
                                                                    &ep_title,
                                                                ) {
                                                                    Ok(_) => success_count += 1,
                                                                    Err(e) => {
                                                                        if first_err.is_empty() {
                                                                            first_err = format!("{}: {}", target.label, e);
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                            if success_count == targets.len() {
                                                                let _ = tx.send(
                                                                    AppMessage::SyncResult {
                                                                        episode_id: ep_id,
                                                                        success: true,
                                                                        message: format!("Copied to {} destination(s)", success_count),
                                                                    },
                                                                );
                                                            } else {
                                                                let _ = tx.send(
                                                                    AppMessage::SyncResult {
                                                                        episode_id: ep_id,
                                                                        success: false,
                                                                        message: first_err,
                                                                    },
                                                                );
                                                            }
                                                        });
                                                    }
                                                }
                                            }

                                            if downloaded {
                                                if ui.small_button("🗑 Delete").clicked() {
                                                    if let Some(ref dpath) = episode.download_path {
                                                        let _ = std::fs::remove_file(dpath);
                                                    }
                                                    let _ = db::clear_download(&self.conn, episode.id);
                                                    self.load_episodes();
                                                }
                                            }

                                            if downloaded {
                                                let play_label = if playing {
                                                    if self.player.is_paused() {
                                                        "▶ Resume"
                                                    } else {
                                                        "⏸ Pause"
                                                    }
                                                } else {
                                                    "▶ Play"
                                                };
                                                if ui.add(egui::Button::new(play_label).small()).clicked() {
                                                    if playing {
                                                        self.player.toggle_play_pause();
                                                    } else if let Some(ref dpath) =
                                                        episode.download_path
                                                    {
                                                        let pos = if episode.played {
                                                            0.0
                                                        } else {
                                                            episode.position_secs
                                                        };
                                                        self.player.load_and_play(
                                                            episode.id,
                                                            dpath,
                                                            pos,
                                                        );
                                                        self.player.set_volume(self.volume);
                                                    }
                                                }
                                            } else if !is_downloading {
                                                if ui.small_button("Download").clicked() {
                                                    let feed = self.feeds.iter().find(|f| {
                                                        f.id == episode.feed_id
                                                    });
                                                    let show_title =
                                                        feed.map(|f| f.title.clone())
                                                            .unwrap_or_default();
                                                    let podcasts_base = PathBuf::from(&self.download_dir);
                                                    let dest_dir = podcasts_base
                                                        .join(sanitize_name(&show_title));
                                                    let dest_path = dest_dir.join(format!(
                                                        "{}.mp3",
                                                        sanitize_name(&episode.title)
                                                    ));

                                                    let tx = self.tx.clone();
                                                    let client = self.client.clone();
                                                    let url = episode.audio_url.clone();
                                                    let ep_id = episode.id;
                                                    let sem = self.download_semaphore.clone();
                                                    self.rt_handle.spawn(async move {
                                                        let _permit = sem.acquire().await;
                                                        download::download_episode(
                                                            &client,
                                                            &url,
                                                            &dest_path,
                                                            ep_id,
                                                            tx,
                                                        )
                                                        .await;
                                                    });
                                                }
                                            }
                                        },
                                    );
                                });

                                if !episode.description.is_empty() {
                                    let desc_text = strip_html(&episode.description);
                                    if is_expanded {
                                        ui.label(
                                            egui::RichText::new(&desc_text)
                                                .size(12.0)
                                                .color(Color32::LIGHT_GRAY),
                                        );
                                        if ui.small_button("Show less").clicked() {
                                            self.expanded_episodes.remove(&episode.id);
                                        }
                                    } else {
                                        let preview: String =
                                            desc_text.chars().take(200).collect();
                                        let preview = if desc_text.len() > 200 {
                                            format!("{}...", preview)
                                        } else {
                                            preview
                                        };
                                        ui.label(
                                            egui::RichText::new(preview)
                                                .size(12.0)
                                                .color(Color32::LIGHT_GRAY),
                                        );
                                        if desc_text.len() > 200
                                            && ui.small_button("Show more").clicked()
                                        {
                                            self.expanded_episodes.insert(episode.id);
                                        }
                                    }
                                }
                            });
                            ui.add_space(4.0);
                        }
                    });
            } else {
                ui.vertical_centered_justified(|ui| {
                    ui.heading("Welcome to RuSStly");
                    ui.label("Add a podcast feed from the sidebar to get started.");
                });
            }
        });

        // --- Bottom Player ---
        egui::TopBottomPanel::bottom("player")
            .min_height(50.0)
            .show(ctx, |ui| {
                if let Some(episode_id) = self.player.current_episode_id() {
                    let episode = self
                        .episodes
                        .iter()
                        .find(|e| e.id == episode_id)
                        .cloned()
                        .or_else(|| {
                            for feed in &self.feeds {
                                if let Ok(eps) = db::get_episodes(&self.conn, feed.id) {
                                    if let Some(ep) = eps.into_iter().find(|e| e.id == episode_id) {
                                        return Some(ep);
                                    }
                                }
                            }
                            None
                        });

                    if let Some(ref episode) = episode {
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.label(
                                    egui::RichText::new(
                                        episode.title.chars().take(60).collect::<String>(),
                                    )
                                    .size(13.0)
                                    .strong(),
                                );
                            });

                            ui.separator();

                            ui.vertical(|ui| {
                                ui.horizontal(|ui| {
                                    if ui
                                        .button(egui::RichText::new("⏪ 15s").size(12.0))
                                        .clicked()
                                    {
                                        self.player.skip_backward(15.0);
                                    }

                                    let play_btn = if self.player.is_paused() {
                                        "▶  Play"
                                    } else if self.player.is_playing() {
                                        "⏸  Pause"
                                    } else {
                                        "▶  Play"
                                    };
                                    if ui
                                        .button(egui::RichText::new(play_btn).size(14.0))
                                        .clicked()
                                    {
                                        self.player.toggle_play_pause();
                                    }

                                    if ui
                                        .button(egui::RichText::new("⏹ Stop").size(12.0))
                                        .clicked()
                                    {
                                        if let Some(eid) = self.player.current_episode_id() {
                                            let pos =
                                                self.player.current_position().as_secs_f64();
                                            let _ = db::update_episode_state(
                                                &self.conn,
                                                eid,
                                                false,
                                                pos,
                                            );
                                        }
                                        self.player.stop();
                                        self.load_episodes();
                                    }

                                    if ui
                                        .button(egui::RichText::new("30s ⏩").size(12.0))
                                        .clicked()
                                    {
                                        self.player.skip_forward(30.0);
                                    }
                                });

                                ui.horizontal(|ui| {
                                    let pos = self.player.current_position();
                                    let pos_secs = pos.as_secs_f64();
                                    ui.label(
                                        egui::RichText::new(format_duration_dur(pos))
                                            .size(11.0)
                                            .monospace(),
                                    );

                                    if let Some(total) = self.player.total_duration() {
                                        let total_secs = total.as_secs_f64();
                                        if total_secs > 0.0 {
                                            let mut seek_pos = pos_secs;
                                            let slider = ui.add_sized(
                                                egui::vec2(ui.available_width().max(100.0), 20.0),
                                                Slider::new(
                                                    &mut seek_pos,
                                                    0.0..=total_secs,
                                                )
                                                .text("")
                                                .show_value(false),
                                            );
                                            if slider.changed() {
                                                self.player.seek_to(
                                                    Duration::from_secs_f64(seek_pos),
                                                );
                                            }
                                        }
                                    } else {
                                        ui.add_space(ui.available_width().max(100.0));
                                    }

                                    let total_str = self
                                        .player
                                        .total_duration()
                                        .map(|d| format_duration_dur(d))
                                        .unwrap_or_else(|| "??:??".to_string());
                                    ui.label(
                                        egui::RichText::new(total_str)
                                            .size(11.0)
                                            .monospace(),
                                    );
                                });
                            });

                            ui.separator();

                            ui.vertical(|ui| {
                                ui.horizontal(|ui| {
                                    ui.label("Speed:");
                                    let speed_values = [0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0];
                                    let current = self.playback_speed;
                                    let mut speed_idx = speed_values
                                        .iter()
                                        .position(|&s| (s - current).abs() < 0.01)
                                        .unwrap_or(2);
                                    egui::ComboBox::new("speed", "")
                                        .selected_text(format!("{:.2}x", current))
                                        .show_ui(ui, |ui| {
                                        for (i, &s) in speed_values.iter().enumerate() {
                                            if ui.selectable_label(
                                                speed_idx == i,
                                                format!("{:.2}x", s),
                                            ).clicked() {
                                                speed_idx = i;
                                                self.playback_speed = s;
                                                self.player.set_speed(s);
                                                let _ = db::set_setting(
                                                    &self.conn,
                                                    "playback_speed",
                                                    &s.to_string(),
                                                );
                                            }
                                        }
                                    });
                                });
                            });

                            ui.separator();

                            ui.vertical(|ui| {
                                ui.horizontal(|ui| {
                                    ui.label("Vol:");
                                    let mut vol = self.volume;
                                    ui.add_sized(
                                        egui::vec2(80.0, 20.0),
                                        Slider::new(&mut vol, 0.0..=1.0)
                                            .text("")
                                            .show_value(false),
                                    );
                                    if vol != self.volume {
                                        self.set_volume(vol);
                                    }
                                    ui.label(format!("{:.0}%", self.volume * 100.0));
                                });
                            });

                            ui.separator();

                            ui.vertical(|ui| {
                                ui.horizontal(|ui| {
                                    let sleep_options = [0, 15, 30, 45, 60, 90];
                                    let label = if self.sleep_timer_minutes > 0 {
                                        if let Some(start) = self.sleep_timer_started_at {
                                            let elapsed = start.elapsed().as_secs_f64();
                                            let remain = (self.sleep_timer_minutes as f64 * 60.0 - elapsed).max(0.0) as u64;
                                            format!("😴 {:02}:{:02}", remain / 60, remain % 60)
                                        } else {
                                            "Sleep".to_string()
                                        }
                                    } else {
                                        "Sleep".to_string()
                                    };
                                    let mut sleep_idx = sleep_options
                                        .iter()
                                        .position(|&s| s == self.sleep_timer_minutes)
                                        .unwrap_or(0);
                                    egui::ComboBox::new("sleep", "")
                                        .selected_text(label)
                                        .show_ui(ui, |ui| {
                                            for (i, &s) in sleep_options.iter().enumerate() {
                                                let name = if s == 0 {
                                                    "Off".to_string()
                                                } else {
                                                    format!("{} min", s)
                                                };
                                                if ui.selectable_label(sleep_idx == i, name).clicked() {
                                                    sleep_idx = i;
                                                    self.sleep_timer_minutes = s;
                                                    if s > 0 {
                                                        self.sleep_timer_started_at = Some(Instant::now());
                                                    } else {
                                                        self.sleep_timer_started_at = None;
                                                    }
                                                }
                                            }
                                        });
                                });
                            });
                        });
                    }
                } else {
                    ui.label("No episode playing");
                }
            });

        // --- Settings Window ---
        if self.show_settings {
            egui::Window::new("Settings")
                .open(&mut self.show_settings)
                .default_width(400.0)
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.heading("OPML Import / Export");
                        ui.separator();

                        ui.label("Import feeds from OPML:");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.opml_import_path)
                                    .hint_text("/path/to/subscriptions.opml")
                                    .desired_width(250.0),
                            );
                            if ui.button("Import").clicked() {
                                match opml::import_opml(&self.opml_import_path) {
                                    Ok(urls) => {
                                        let mut count = 0;
                                        for url in &urls {
                                            if !self.feeds.iter().any(|f| f.url == *url) {
                                                if let Ok(feed_id) = db::add_feed(
                                                    &self.conn,
                                                    url,
                                                    "Importing...",
                                                    "",
                                                    "",
                                                ) {
                                                    count += 1;
                                                    let tx = self.tx.clone();
                                                    let client = self.client.clone();
                                                    let url = url.clone();
                                                    self.rt_handle.spawn(async move {
                                                        if let Ok((title, description, image_url, episodes)) =
                                                            feed::fetch_feed(&client, &url).await
                                                        {
                                                            let _ = tx.send(
                                                                AppMessage::FeedFetched {
                                                                    feed_id,
                                                                    title,
                                                                    description,
                                                                    image_url,
                                                                    episodes,
                                                                },
                                                            );
                                                        }
                                                    });
                                                }
                                            }
                                        }
                                        self.feeds = db::get_feeds(&self.conn).unwrap_or_default();
                                        self.opml_status = format!(
                                            "Imported {} feeds from OPML",
                                            count
                                        );
                                    }
                                    Err(e) => {
                                        self.opml_status = format!("OPML import failed: {}", e);
                                    }
                                }
                            }
                        });

                        ui.label("Export feeds to OPML:");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.opml_export_path)
                                    .hint_text("/path/to/subscriptions.opml")
                                    .desired_width(250.0),
                            );
                            if ui.button("Export").clicked() {
                                match opml::export_opml(&self.opml_export_path, &self.feeds) {
                                    Ok(()) => {
                                        self.opml_status = format!(
                                            "Exported {} feeds to OPML",
                                            self.feeds.len()
                                        );
                                    }
                                    Err(e) => {
                                        self.opml_status = format!("OPML export failed: {}", e);
                                    }
                                }
                            }
                        });

                        if !self.opml_status.is_empty() {
                            ui.colored_label(Color32::GREEN, &self.opml_status);
                        }

                        ui.add_space(10.0);
                        ui.heading("Downloads");
                        ui.separator();

                        ui.label("Download directory:");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.download_dir)
                                    .desired_width(300.0),
                            );
                            if ui.button("Browse").clicked() {
                                // File dialog would go here with rfd
                            }
                        });
                        if self.download_dir != db::get_setting(&self.conn, "download_dir").unwrap_or_default() {
                            let _ = db::set_setting(&self.conn, "download_dir", &self.download_dir);
                        }

                        ui.checkbox(&mut self.auto_download, "Auto-download new episodes");
                        if self.auto_download != (db::get_setting(&self.conn, "auto_download").map(|v| v == "true").unwrap_or(false)) {
                            let val = if self.auto_download { "true" } else { "false" };
                            let _ = db::set_setting(&self.conn, "auto_download", val);
                        }

                        ui.horizontal(|ui| {
                            ui.label("Max episodes per feed:");
                            let mut max = self.max_episodes as i32;
                            if ui.add(egui::Slider::new(&mut max, 5..=300).text(""))
                                .changed()
                            {
                                self.max_episodes = max as usize;
                                let _ = db::set_setting(
                                    &self.conn,
                                    "max_episodes",
                                    &self.max_episodes.to_string(),
                                );
                            }
                        });

                        ui.add_space(10.0);
                        ui.heading("Sync Targets");
                        ui.separator();

                        let targets = self.sync_targets.clone();
                        for target in &targets {
                            ui.horizontal(|ui| {
                                ui.label(&target.label);
                                ui.label(egui::RichText::new(&target.path).size(11.0).color(Color32::GRAY));
                                if ui.small_button("Remove").clicked() {
                                    let _ = db::remove_sync_target(&self.conn, target.id);
                                    self.sync_targets = db::get_sync_targets(&self.conn).unwrap_or_default();
                                }
                            });
                        }

                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.new_sync_label)
                                    .hint_text("Label")
                                    .desired_width(100.0),
                            );
                            ui.add(
                                egui::TextEdit::singleline(&mut self.new_sync_path)
                                    .hint_text("/path/to/destination")
                                    .desired_width(200.0),
                            );
                            if ui.button("Add").clicked() {
                                let label = self.new_sync_label.clone();
                                let path = self.new_sync_path.clone();
                                if !label.is_empty() && !path.is_empty() {
                                    let _ = db::add_sync_target(&self.conn, &label, &path);
                                    self.sync_targets = db::get_sync_targets(&self.conn).unwrap_or_default();
                                    self.new_sync_label.clear();
                                    self.new_sync_path.clear();
                                }
                            }
                        });

                        ui.add_space(10.0);
                        ui.heading("Playback");
                        ui.separator();

                        ui.checkbox(&mut self.auto_mark_played, "Auto-mark as played near end");
                        if self.auto_mark_played != (db::get_setting(&self.conn, "auto_mark_played").map(|v| v == "true").unwrap_or(true)) {
                            let val = if self.auto_mark_played { "true" } else { "false" };
                            let _ = db::set_setting(&self.conn, "auto_mark_played", val);
                        }

                        if self.auto_mark_played {
                            ui.horizontal(|ui| {
                                ui.label("Mark as played at:");
                                let mut pct = (self.auto_mark_threshold * 100.0) as i32;
                                if ui
                                    .add(egui::Slider::new(&mut pct, 50..=100).text("%"))
                                    .changed()
                                {
                                    self.auto_mark_threshold = pct as f32 / 100.0;
                                    let _ = db::set_setting(
                                        &self.conn,
                                        "auto_mark_threshold",
                                        &self.auto_mark_threshold.to_string(),
                                    );
                                }
                            });
                        }

                        ui.add_space(10.0);
                        ui.heading("Appearance");
                        ui.separator();

                        ui.horizontal(|ui| {
                            ui.label("Theme:");
                            let dark_label = if self.dark_mode { "☾ Dark" } else { "☀ Light" };
                            if ui.button(dark_label).clicked() {
                                self.dark_mode = !self.dark_mode;
                                let val = if self.dark_mode { "true" } else { "false" };
                                let _ = db::set_setting(&self.conn, "dark_mode", val);
                            }
                        });

                        ui.add_space(10.0);
                        ui.heading("About");
                        ui.separator();
                        ui.label("RuSStly — Rust Podcast Client");
                        ui.label("Built with eframe/egui, rodio, reqwest, rusqlite");
                    });
                });
        }

        // Tray event handling
        if let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == self.tray_show_id {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            } else if event.id == self.tray_quit_id {
                self.should_quit = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }

        // Handle close request (user clicked X)
        if ctx.input(|i| i.viewport().close_requested()) {
            if self.should_quit || self.download_progress.is_empty() {
                
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            }
        }

        if self.should_hide {
            self.should_hide = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }

        if self.should_quit && !self.download_progress.is_empty() {
            self.should_quit = false;
            self.should_hide = true;
        }
    }
}

fn send_notification(summary: &str, body: &str) -> Result<(), Box<dyn std::error::Error>> {
    Notification::new()
        .summary(summary)
        .body(body)
        .appname("RuSStly")
        .show()?;
    Ok(())
}
