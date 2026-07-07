use std::io::BufReader;
use std::time::Duration;

use rss::Channel;

use crate::types::NewEpisode;

pub async fn fetch_feed(
    client: &reqwest::Client,
    url: &str,
) -> Result<(String, String, String, Vec<NewEpisode>), String> {
    let response = client
        .get(url)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| {
            format!("HTTP request failed: {}", e)
        })?;

    let bytes = response
        .bytes()
        .await
        .map_err(|e| {
            format!("Failed to read response: {}", e)
        })?;

    let channel = Channel::read_from(BufReader::new(&bytes[..]))
        .map_err(|e| {
            format!("Failed to parse RSS: {}", e)
        })?;

    let title = channel.title().to_string();
    let description = channel.description().to_string();
    let image_url = channel
        .image()
        .map(|img| img.url().to_string())
        .unwrap_or_default();

    let mut episodes = Vec::new();
    for item in channel.items() {
        let guid = item
            .guid()
            .map(|g| g.value().to_string())
            .unwrap_or_default();
        let guid = if guid.is_empty() {
            item.link().unwrap_or("").to_string()
        } else {
            guid
        };
        let ep_title = item.title().unwrap_or("").to_string();
        let ep_desc = item.description().unwrap_or("").to_string();
        let pub_date = item.pub_date().unwrap_or("").to_string();

        let mut audio_url = String::new();
        if let Some(enclosure) = item.enclosure() {
            let mime = enclosure.mime_type();
            if mime.starts_with("audio/")
                || enclosure.url().ends_with(".mp3")
                || enclosure.url().ends_with(".m4a")
                || enclosure.url().ends_with(".ogg")
            {
                audio_url = enclosure.url().to_string();
            }
        }

        let mut duration_secs = None;
        if let Some(extensions) = item.extensions().get("itunes") {
            if let Some(duration_exts) = extensions.get("duration") {
                if let Some(ext) = duration_exts.first() {
                    if let Some(val) = ext.value.as_ref() {
                        duration_secs = parse_duration(val);
                    }
                }
            }
        }

            if !audio_url.is_empty() {
            episodes.push(NewEpisode {
                feed_id: 0,
                guid,
                title: ep_title,
                description: ep_desc,
                pub_date,
                duration_secs,
                audio_url,
            });
        }
    }

    Ok((title, description, image_url, episodes))
}

fn parse_duration(s: &str) -> Option<i64> {
    let s = s.trim();
    if let Ok(secs) = s.parse::<i64>() {
        return Some(secs);
    }
    let parts: Vec<&str> = s.split(':').collect();
    match parts.len() {
        3 => {
            let h = parts[0].parse::<i64>().ok()?;
            let m = parts[1].parse::<i64>().ok()?;
            let s = parts[2].parse::<i64>().ok()?;
            Some(h * 3600 + m * 60 + s)
        }
        2 => {
            let m = parts[0].parse::<i64>().ok()?;
            let s = parts[1].parse::<i64>().ok()?;
            Some(m * 60 + s)
        }
        _ => None,
    }
}
