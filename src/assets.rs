use axum::{
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "frontend/"]
struct FrontendAssets;

/// 静的ファイル配信ハンドラ
pub async fn serve_static(axum::extract::Path(path): axum::extract::Path<String>) -> Response {
    serve_file(&path)
}

/// index.html 配信
pub async fn serve_index() -> Response {
    serve_file("index.html")
}

fn serve_file(path: &str) -> Response {
    match FrontendAssets::get(path) {
        Some(file) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            // Cache-Control: index.html は短め、それ以外は長め
            let cache_control = if path == "index.html" {
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
