use rusqlite::{params, Connection, Result};

use crate::types::{Episode, Feed, NewEpisode};

pub fn init_db(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS feeds (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            url TEXT NOT NULL UNIQUE,
            title TEXT NOT NULL DEFAULT '',
            description TEXT NOT NULL DEFAULT '',
            image_url TEXT NOT NULL DEFAULT ''
        );
        CREATE TABLE IF NOT EXISTS episodes (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            feed_id INTEGER NOT NULL,
            guid TEXT NOT NULL DEFAULT '',
            title TEXT NOT NULL DEFAULT '',
            description TEXT NOT NULL DEFAULT '',
            pub_date TEXT NOT NULL DEFAULT '',
            duration_secs INTEGER,
            audio_url TEXT NOT NULL DEFAULT '',
            played INTEGER NOT NULL DEFAULT 0,
            downloaded INTEGER NOT NULL DEFAULT 0,
            download_path TEXT,
            position_secs REAL NOT NULL DEFAULT 0.0,
            FOREIGN KEY(feed_id) REFERENCES feeds(id) ON DELETE CASCADE
        );
        CREATE UNIQUE INDEX IF NOT EXISTS idx_episodes_feed_guid ON episodes(feed_id, guid);
        CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );",
    )?;
    Ok(())
}

pub fn add_feed(conn: &Connection, url: &str, title: &str, description: &str, image_url: &str) -> Result<i64> {
    conn.execute(
        "INSERT INTO feeds (url, title, description, image_url) VALUES (?1, ?2, ?3, ?4)",
        params![url, title, description, image_url],
    )?;
    let id = conn.last_insert_rowid();
    Ok(id)
}

pub fn get_feeds(conn: &Connection) -> Result<Vec<Feed>> {
    let mut stmt = conn.prepare(
        "SELECT id, url, title, description, image_url FROM feeds ORDER BY title",
    )?;
    let feeds = stmt
        .query_map([], |row| {
            Ok(Feed {
                id: row.get(0)?,
                url: row.get(1)?,
                title: row.get(2)?,
                description: row.get(3)?,
                image_url: row.get(4)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(feeds)
}

pub fn remove_feed(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM episodes WHERE feed_id = ?1", params![id])?;
    conn.execute("DELETE FROM feeds WHERE id = ?1", params![id])?;
    Ok(())
}

pub fn get_episodes(conn: &Connection, feed_id: i64) -> Result<Vec<Episode>> {
    let mut stmt = conn.prepare(
        "SELECT id, feed_id, guid, title, description, pub_date, duration_secs, \
         audio_url, played, downloaded, download_path, position_secs \
         FROM episodes WHERE feed_id = ?1 ORDER BY pub_date DESC",
    )?;
    let episodes = stmt
        .query_map(params![feed_id], |row| {
            Ok(Episode {
                id: row.get(0)?,
                feed_id: row.get(1)?,
                guid: row.get(2)?,
                title: row.get(3)?,
                description: row.get(4)?,
                pub_date: row.get(5)?,
                duration_secs: row.get(6)?,
                audio_url: row.get(7)?,
                played: row.get::<_, i32>(8)? != 0,
                downloaded: row.get::<_, i32>(9)? != 0,
                download_path: row.get(10)?,
                position_secs: row.get(11)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(episodes)
}

pub fn upsert_episodes_batch(conn: &mut Connection, feed_id: i64, episodes: &[NewEpisode]) -> Result<Vec<(i64, bool)>> {
    let mut results = Vec::with_capacity(episodes.len());
    let tx = conn.transaction()?;
    for ep in episodes {
        let existing = tx.query_row(
            "SELECT id, downloaded FROM episodes WHERE feed_id = ?1 AND guid = ?2",
            params![feed_id, ep.guid],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i32>(1)? != 0)),
        );
        match existing {
            Ok((id, was_downloaded)) => {
                tx.execute(
                    "UPDATE episodes SET title=?1, description=?2, pub_date=?3, \
                     duration_secs=?4, audio_url=?5 WHERE id=?6",
                    params![ep.title, ep.description, ep.pub_date, ep.duration_secs, ep.audio_url, id],
                )?;
                results.push((id, was_downloaded));
            }
            Err(_) => {
                tx.execute(
                    "INSERT INTO episodes (feed_id, guid, title, description, pub_date, \
                     duration_secs, audio_url) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        feed_id,
                        ep.guid,
                        ep.title,
                        ep.description,
                        ep.pub_date,
                        ep.duration_secs,
                        ep.audio_url,
                    ],
                )?;
                results.push((tx.last_insert_rowid(), false));
            }
        }
    }
    tx.commit()?;
    Ok(results)
}

pub fn update_episode_state(conn: &Connection, episode_id: i64, played: bool, position_secs: f64) -> Result<()> {
    conn.execute(
        "UPDATE episodes SET played = ?1, position_secs = ?2 WHERE id = ?3",
        params![played as i32, position_secs, episode_id],
    )?;
    Ok(())
}

pub fn set_episode_downloaded(conn: &Connection, episode_id: i64, path: &str) -> Result<()> {
    conn.execute(
        "UPDATE episodes SET downloaded = 1, download_path = ?1 WHERE id = ?2",
        params![path, episode_id],
    )?;
    Ok(())
}

pub fn update_feed(conn: &Connection, id: i64, title: &str, description: &str, image_url: &str) -> Result<()> {
    conn.execute(
        "UPDATE feeds SET title = ?1, description = ?2, image_url = ?3 WHERE id = ?4",
        params![title, description, image_url, id],
    )?;
    Ok(())
}

pub fn set_episode_played(conn: &Connection, episode_id: i64) -> Result<()> {
    conn.execute(
        "UPDATE episodes SET played = 1 WHERE id = ?1",
        params![episode_id],
    )?;
    Ok(())
}

pub fn clear_download(conn: &Connection, episode_id: i64) -> Result<()> {
    conn.execute(
        "UPDATE episodes SET downloaded = 0, download_path = NULL WHERE id = ?1",
        params![episode_id],
    )?;
    Ok(())
}

pub fn get_setting(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        params![key],
        |row| row.get(0),
    )
    .ok()
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}
