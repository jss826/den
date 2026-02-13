use axum::{
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "frontend/"]
struct FrontendAssets;

/// 静的ファイル配信ハンドラ
pub async fn serve_static(
    axum::extract::Path(path): axum::extract::Path<String>,
) -> Response {
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
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, mime.as_ref().to_string())],
                file.data.to_vec(),
            )
                .into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
