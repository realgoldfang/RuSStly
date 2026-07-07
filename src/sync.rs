use std::path::Path;

pub fn sync_episode(source_path: &Path, target_dir: &Path, show_title: &str, episode_title: &str) -> Result<String, String> {
    let show_dir = target_dir.join(sanitize_filename(show_title));
    std::fs::create_dir_all(&show_dir)
        .map_err(|e| {
            format!("Failed to create device directory: {}", e)
        })?;

    let extension = source_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("mp3");
    let dest_filename = format!("{}.{}", sanitize_filename(episode_title), extension);
    let dest_path = show_dir.join(&dest_filename);

    std::fs::copy(source_path, &dest_path)
        .map_err(|e| {
            format!("Failed to copy file: {}", e)
        })?;
    Ok(dest_path.to_string_lossy().to_string())
}

fn sanitize_filename(name: &str) -> String {
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
