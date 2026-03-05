use axum::{
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use rust_embed::Embed;
use std::sync::OnceLock;

#[derive(Embed)]
#[folder = "frontend/"]
struct FrontendAssets;

/// Cache-busted index.html body + ETag (built once, reused for all requests)
static CACHED_INDEX: OnceLock<(Bytes, String)> = OnceLock::new();

/// Build index.html with cache-busting query parameters on JS/CSS URLs.
fn build_index_html() -> (Bytes, String) {
    let file = FrontendAssets::get("index.html").expect("index.html must exist in frontend/");
    let html = String::from_utf8_lossy(&file.data);

    // Collect hash prefixes for all embedded assets
    let mut replacements: Vec<(String, String)> = Vec::new();
    for path in FrontendAssets::iter() {
        let path_str = path.as_ref();
        if (path_str.ends_with(".js") || path_str.ends_with(".css"))
            && let Some(asset) = FrontendAssets::get(path_str)
        {
            let hash = hex::encode(asset.metadata.sha256_hash());
            let short_hash = &hash[..8];
            let url = format!("/{path_str}");
            let busted = format!("/{path_str}?v={short_hash}");
            replacements.push((url, busted));
        }
    }

    let mut result = html.into_owned();
    for (from, to) in &replacements {
        result = result.replace(from, to);
    }

    // ETag from the transformed content (not the original)
    let etag = format!("\"{}\"", &hex::encode(sha2_digest(result.as_bytes()))[..16]);
    (Bytes::from(result), etag)
}

fn sha2_digest(data: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().into()
}

/// 静的ファイル配信ハンドラ
pub async fn serve_static(axum::extract::Path(path): axum::extract::Path<String>) -> Response {
    serve_file(&path)
}

/// index.html 配信
pub async fn serve_index() -> Response {
    let (body, etag) = CACHED_INDEX.get_or_init(build_index_html);

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8".to_string()),
            (header::CACHE_CONTROL, "public, max-age=60".to_string()),
            (header::ETAG, etag.clone()),
        ],
        body.clone(),
    )
        .into_response()
}

fn serve_file(path: &str) -> Response {
    match FrontendAssets::get(path) {
        Some(file) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            // JS/CSS use immutable caching (URLs contain hash via cache-busting)
            let cache_control = if path.ends_with(".js") || path.ends_with(".css") {
                "public, max-age=31536000, immutable"
            } else if path == "index.html" {
                "public, max-age=60"
            } else {
                "public, max-age=86400"
            };
            // ETag: rust-embed のハッシュを利用
            let etag = hex::encode(file.metadata.sha256_hash());
            // Cow を直接 Body に変換（Borrowed は zero-copy）
            let body: Bytes = match file.data {
                std::borrow::Cow::Borrowed(b) => Bytes::from_static(b),
                std::borrow::Cow::Owned(v) => Bytes::from(v),
            };
            (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, mime.as_ref().to_string()),
                    (header::CACHE_CONTROL, cache_control.to_string()),
                    (header::ETAG, format!("\"{}\"", etag)),
                ],
                body,
            )
                .into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
