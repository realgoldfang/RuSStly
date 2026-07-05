
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct Feed {
    pub id: i64,
    pub url: String,
    pub title: String,
    pub description: String,
    pub image_url: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct Episode {
    pub id: i64,
    pub feed_id: i64,
    pub guid: String,
    pub title: String,
    pub description: String,
    pub pub_date: String,
    pub duration_secs: Option<i64>,
    pub audio_url: String,
    pub played: bool,
    pub downloaded: bool,
    pub download_path: Option<String>,
    pub position_secs: f64,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct NewEpisode {
    pub feed_id: i64,
    pub guid: String,
    pub title: String,
    pub description: String,
    pub pub_date: String,
    pub duration_secs: Option<i64>,
    pub audio_url: String,
}

#[derive(Debug)]
pub enum AppMessage {
    FeedFetched {
        feed_id: i64,
        title: String,
        description: String,
        image_url: String,
        episodes: Vec<NewEpisode>,
    },
    FeedFetchFailed {
        url: String,
        error: String,
    },
    DownloadProgress {
        episode_id: i64,
        progress: f64,
    },
    DownloadComplete {
        episode_id: i64,
        path: String,
    },
    DownloadFailed {
        episode_id: i64,
        error: String,
    },
    SyncResult {
        episode_id: i64,
        success: bool,
        message: String,
    },
}
