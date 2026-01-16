//! HTML index generator for offline bundles.

use super::types::ManifestItem;

/// Escape HTML special characters
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Generate HTML index page for the bundle
pub fn generate_index(items: &[ManifestItem], created: &str) -> String {
    let mut html = String::from(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Offline Bundle</title>
    <style>
        * { box-sizing: border-box; margin: 0; padding: 0; }
        body {
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            background: #1a1a2e;
            color: #eee;
            padding: 16px;
            min-height: 100vh;
        }
        h1 {
            text-align: center;
            margin-bottom: 6px;
            color: #e94560;
            font-size: 1.3rem;
        }
        .created {
            text-align: center;
            color: #555;
            font-size: 0.75rem;
            margin-bottom: 16px;
        }
        .list {
            max-width: 700px;
            margin: 0 auto;
        }
        .item {
            display: flex;
            align-items: flex-start;
            gap: 10px;
            padding: 10px 12px;
            background: #16213e;
            border-radius: 8px;
            margin-bottom: 6px;
            text-decoration: none;
            color: inherit;
        }
        .item:active {
            background: #1e2a4a;
        }
        .icon {
            font-size: 1.2rem;
            flex-shrink: 0;
            line-height: 1.4;
        }
        .info {
            flex: 1;
            min-width: 0;
        }
        .title {
            font-size: 0.9rem;
            font-weight: 500;
            color: #fff;
            line-height: 1.35;
        }
        .domain {
            font-size: 0.7rem;
            color: #666;
            margin-top: 3px;
        }
        .note {
            text-align: center;
            color: #444;
            margin-top: 24px;
            font-size: 0.7rem;
        }
    </style>
</head>
<body>
    <h1>Offline Bundle</h1>
"#,
    );

    // Add created date
    html.push_str(&format!(
        "    <div class=\"created\">{}</div>\n    <div class=\"list\">\n",
        html_escape(created)
    ));

    for item in items {
        let icon = match item.item_type.as_str() {
            "article" => "📄",
            "video" => "🎬",
            "pdf" => "📕",
            _ => "📄",
        };

        html.push_str(&format!(
            r#"        <a class="item" href="{}">
            <span class="icon">{}</span>
            <div class="info">
                <div class="title">{}</div>
                <div class="domain">{}</div>
            </div>
        </a>
"#,
            html_escape(&item.local_path),
            icon,
            html_escape(&item.title),
            html_escape(&item.domain),
        ));
    }

    html.push_str(
        r#"    </div>
    <p class="note">Tap to open · Videos→VLC · EPUBs/PDFs→Books</p>
</body>
</html>"#,
    );

    html
}
