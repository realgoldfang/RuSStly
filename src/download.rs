use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use tokio::fs::File;
use tokio::io::AsyncWriteExt;

use crate::types::AppMessage;

pub async fn download_episode(
    client: &reqwest::Client,
    url: &str,
    dest_path: &Path,
    episode_id: i64,
    tx: mpsc::Sender<AppMessage>,
) {
    let result = download_episode_inner(client, url, dest_path, episode_id, &tx).await;
    if let Err(e) = result {
        let _ = tx.send(AppMessage::DownloadFailed {
            episode_id,
            error: e,
        });
    }
}

async fn download_episode_inner(
    client: &reqwest::Client,
    url: &str,
    dest_path: &Path,
    episode_id: i64,
    tx: &mpsc::Sender<AppMessage>,
) -> Result<(), String> {
    if let Some(parent) = dest_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("Failed to create directory: {}", e))?;
    }

    let response = client
        .get(url)
        .timeout(Duration::from_secs(120))
        .send()
        .await
        .map_err(|e| format!("Download request failed: {}", e))?;

    let total = response.content_length().unwrap_or(0);
    let mut downloaded: u64 = 0;

    let mut file = File::create(dest_path)
        .await
        .map_err(|e| format!("Failed to create file: {}", e))?;

    let mut response = response;
    loop {
        let chunk = response
            .chunk()
            .await
            .map_err(|e| format!("Download stream error: {}", e))?;
        match chunk {
            Some(data) => {
                file.write_all(&data)
                    .await
                    .map_err(|e| format!("Failed to write file: {}", e))?;
                downloaded += data.len() as u64;
                if total > 0 {
                    let progress = downloaded as f64 / total as f64;
                    let _ = tx.send(AppMessage::DownloadProgress {
                        episode_id,
                        progress: progress.min(1.0),
                    });
                }
            }
            None => break,
        }
    }

    file.flush()
        .await
        .map_err(|e| format!("Failed to flush file: {}", e))?;

    let _ = tx.send(AppMessage::DownloadComplete {
        episode_id,
        path: dest_path.to_string_lossy().to_string(),
    });

    Ok(())
}
