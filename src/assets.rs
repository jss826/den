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
    // /index.html must return the cache-busted version, same as /
    if path == "index.html" {
        return serve_index().await;
    }
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

/// multiplexer 用 layout/config を `data_dir` に書き出し、各絶対パスを返す。
/// `shell`（Den の設定シェル）は zellij `default_shell` / tmux `default-command` の
/// `__DEN_SHELL__` プレースホルダへ展開され、mux セッションのシェルを plain Den
/// セッションと一致させる（PSReadLine/readline の Ctrl+R 等の挙動を揃える）。
/// 書き出し失敗・asset 欠落時は warn ログを出し、そのパスは空文字列を返す
/// （呼び出し側は該当フラグ省略にフォールバックする）。
pub fn ensure_mux_layouts(
    data_dir: &std::path::Path,
    shell: &str,
) -> crate::pty::backend::MuxConfig {
    // config 値（二重引用符内）へ安全に埋め込む。
    // shell は DEN_SHELL（operator 制御）由来だが、改行・制御文字が混じると
    // クォートを抜けて KDL/tmux のディレクティブを注入し得るため、まず制御文字を
    // 除去し、その上で `\` と `"` を打ち消す（Windows フルパスのバックスラッシュ対策）。
    let shell_escaped = shell
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");

    // テンプレート（`__DEN_SHELL__` 置換）を施して書き出す。
    fn write_templated(
        data_dir: &std::path::Path,
        embedded: &str,
        out_name: &str,
        shell_escaped: &str,
    ) -> String {
        write_rendered(data_dir, embedded, out_name, &|text| {
            text.replace("__DEN_SHELL__", shell_escaped)
        })
    }
    fn write_rendered(
        data_dir: &std::path::Path,
        embedded: &str,
        out_name: &str,
        render: &dyn Fn(&str) -> String,
    ) -> String {
        let path = data_dir.join(out_name);
        match FrontendAssets::get(embedded) {
            Some(file) => {
                let rendered = render(&String::from_utf8_lossy(file.data.as_ref()));
                match std::fs::write(&path, rendered.as_bytes()) {
                    Ok(()) => path.to_string_lossy().into_owned(),
                    Err(e) => {
                        tracing::warn!("Failed to write {out_name}: {e}");
                        String::new()
                    }
                }
            }
            None => {
                tracing::warn!("Embedded asset {embedded} missing");
                String::new()
            }
        }
    }

    crate::pty::backend::MuxConfig {
        zellij_config: write_templated(
            data_dir,
            "layouts/den-zellij.kdl",
            "den-zellij.kdl",
            &shell_escaped,
        ),
        tmux_conf: write_templated(data_dir, "layouts/den.conf", "den.conf", &shell_escaped),
    }
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

#[cfg(test)]
mod mux_layout_tests {
    use super::*;

    #[test]
    fn ensure_mux_layouts_writes_files() {
        let dir = std::env::temp_dir().join("den-mux-layout-test");
        let _ = std::fs::create_dir_all(&dir);
        let mux = ensure_mux_layouts(&dir, "powershell.exe");
        assert!(std::path::Path::new(&mux.zellij_config).exists());
        assert!(std::path::Path::new(&mux.tmux_conf).exists());

        let conf_body = std::fs::read_to_string(&mux.tmux_conf).expect("conf readable");
        // Native 化: status line は出す（status off を書かない）
        assert!(!conf_body.contains("status off"));
        assert!(conf_body.contains("set -g window-size latest"));
        // tmux: shell 展開 ＋ prefix 解放は維持
        assert!(conf_body.contains("default-command \"powershell.exe\""));
        assert!(conf_body.contains("set -g prefix None"));

        let cfg_body = std::fs::read_to_string(&mux.zellij_config).expect("cfg readable");
        // zellij: default_shell 展開 ＋ keybinds clear-defaults は維持
        assert!(cfg_body.contains("default_shell \"powershell.exe\""));
        assert!(cfg_body.contains("clear-defaults=true"));
        assert!(!cfg_body.contains("__DEN_SHELL__"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ensure_mux_layouts_escapes_backslashes_in_shell() {
        // Windows フルパス（バックスラッシュ）でも KDL/conf が壊れない
        let dir = std::env::temp_dir().join("den-mux-layout-esc-test");
        let _ = std::fs::create_dir_all(&dir);
        let mux = ensure_mux_layouts(&dir, r"C:\Win\pwsh.exe");
        let cfg_body = std::fs::read_to_string(&mux.zellij_config).expect("cfg readable");
        assert!(cfg_body.contains(r#"default_shell "C:\\Win\\pwsh.exe""#));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ensure_mux_layouts_strips_control_chars_from_shell() {
        // 改行/制御文字は除去され、クォートを抜けたディレクティブ注入を防ぐ
        let dir = std::env::temp_dir().join("den-mux-layout-ctrl-test");
        let _ = std::fs::create_dir_all(&dir);
        let mux = ensure_mux_layouts(&dir, "sh\"\nkeybinds { x }\npwsh");
        let cfg_body = std::fs::read_to_string(&mux.zellij_config).expect("cfg readable");
        // 制御文字（改行）が落ちて 1 行・1 ディレクティブに収まる
        assert!(cfg_body.contains(r#"default_shell "sh\"keybinds { x }pwsh""#));
        assert!(!cfg_body.contains("\nkeybinds { x }"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
