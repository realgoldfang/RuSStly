use crate::types::Feed;

pub fn import_opml(path: &str) -> Result<Vec<String>, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| {
            format!("Failed to read OPML file: {}", e)
        })?;

    let mut urls = Vec::new();
    let search = "xmlUrl=\"";

    let mut pos = 0;
    while let Some(start) = content[pos..].find(search) {
        let start = pos + start + search.len();
        if let Some(end) = content[start..].find('"') {
            let url = &content[start..start + end];
            let url = url.trim();
            if !url.is_empty() {
                urls.push(url.to_string());
            }
            pos = start + end + 1;
        } else {
            break;
        }
    }

    if urls.is_empty() {
        return Err("No feed URLs found in OPML file".to_string());
    }

    Ok(urls)
}

pub fn export_opml(path: &str, feeds: &[Feed]) -> Result<(), String> {
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <opml version=\"2.0\">\n\
         <head>\n\
         <title>RuSStly Subscriptions</title>\n\
         </head>\n\
         <body>\n",
    );

    for feed in feeds {
        xml.push_str(&format!(
            "<outline text=\"{}\" title=\"{}\" type=\"rss\" xmlUrl=\"{}\" />\n",
            escape_xml(&feed.title),
            escape_xml(&feed.title),
            escape_xml(&feed.url),
        ));
    }

    xml.push_str("</body>\n</opml>\n");

    std::fs::write(path, &xml).map_err(|e| format!("Failed to write OPML: {}", e))?;

    Ok(())
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
