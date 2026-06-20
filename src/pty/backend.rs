use serde::{Deserialize, Serialize};
use std::process::Command;

/// セッション起動の backend 種別
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SessionBackend {
    #[default]
    Shell,
    Zellij,
    Tmux,
}

/// multiplexer 起動に使う materialized なファイルパス群。
/// `ensure_mux_layouts` が `data_dir` へ書き出した絶対パスを保持する。
/// 各フィールドが空文字列 = 書き出し失敗（`build_launch_command` は該当フラグを省略）。
#[derive(Debug, Clone, Default)]
pub struct MuxConfig {
    /// zellij bare layout（`-l`）
    pub zellij_layout: String,
    /// zellij config（`--config`）: default_shell ＋ keybinds clear-defaults
    pub zellij_config: String,
    /// tmux conf（`-f`）
    pub tmux_conf: String,
}

/// backend に応じた起動コマンド (program, args) を組み立てる。
/// シェル文字列連結はしない（argv 配列で CommandBuilder に渡す）。
/// name は is_valid_session_name で英数＋`-` に限定済みの前提。
///
/// zellij/tmux の argv は Task 1 PoC findings の確定形を使う:
/// - zellij: `zellij --config <cfg> -l <layout> attach -c <name>`（attach-or-create。
///   `--config` で Den 専用設定[default_shell ＋ keybinds clear-defaults]を渡す。
///   グローバルオプションなのでサブコマンド `attach -c` より前に置く。long form 必須
///   [`attach -c` の `-c`=create と短縮形が衝突するため]）
/// - tmux:   `tmux -f <conf> new-session -A -s <name>`（`-A` で attach-or-create）
///
/// layout/config/conf パスが空（embed 書き出し失敗）のときは該当フラグを最初から付けず、
/// 素の `zellij attach -c <name>` / `tmux new-session -A -s <name>` を返す。
/// 空文字列を引数として渡したり、後段で文字列一致で除去したりはしない
/// （name が偶然 `-l`/`-f` と一致しても壊れない）。
pub fn build_launch_command(
    backend: SessionBackend,
    shell: &str,
    name: &str,
    mux: &MuxConfig,
) -> (String, Vec<String>) {
    match backend {
        SessionBackend::Shell => (shell.to_string(), Vec::new()),
        SessionBackend::Zellij => {
            let mut args = Vec::new();
            // --config はグローバルオプション。サブコマンド前・long form で渡す。
            if !mux.zellij_config.is_empty() {
                args.push("--config".to_string());
                args.push(mux.zellij_config.clone());
            }
            if !mux.zellij_layout.is_empty() {
                args.push("-l".to_string());
                args.push(mux.zellij_layout.clone());
            }
            args.push("attach".to_string());
            args.push("-c".to_string());
            args.push(name.to_string());
            ("zellij".to_string(), args)
        }
        SessionBackend::Tmux => {
            let mut args = Vec::new();
            if !mux.tmux_conf.is_empty() {
                args.push("-f".to_string());
                args.push(mux.tmux_conf.clone());
            }
            args.push("new-session".to_string());
            args.push("-A".to_string());
            args.push("-s".to_string());
            args.push(name.to_string());
            ("tmux".to_string(), args)
        }
    }
}

/// `tmux ls`（`name: N windows (...)` 形式）からセッション名を抽出する。
pub fn parse_tmux_ls(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| line.split(':').next())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// `zellij list-sessions --short --no-formatting`（名前のみ・1 行 1 名）から
/// セッション名を抽出する。装飾フラグなしでも防御的に ANSI を落とし行頭トークンを取る。
pub fn parse_zellij_ls(output: &str) -> Vec<String> {
    output
        .lines()
        .map(strip_ansi)
        .filter_map(|line| line.split_whitespace().next().map(str::to_string))
        .filter(|s| !s.is_empty())
        .collect()
}

/// 簡易 ANSI ストリップ（ESC[...m を読み飛ばす）。
fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for n in chars.by_ref() {
                if n == 'm' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// backend の実行ファイルが PATH 上にあるか（`--version`/`-V` の成功で判定）。
pub fn probe_available(backend: SessionBackend) -> bool {
    let (prog, flag) = match backend {
        SessionBackend::Shell => return true,
        SessionBackend::Zellij => ("zellij", "--version"),
        SessionBackend::Tmux => ("tmux", "-V"),
    };
    Command::new(prog)
        .arg(flag)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// backend の既存セッション名一覧を返す（ls 実行 → パース、失敗時は空）。
/// 注意: blocking なので呼び出し側は spawn_blocking で囲むこと。
pub fn list_mux_sessions(backend: SessionBackend) -> Vec<String> {
    let (prog, args): (&str, &[&str]) = match backend {
        SessionBackend::Shell => return Vec::new(),
        // PoC findings §2: --short --no-formatting で名前のみが得られる
        SessionBackend::Zellij => ("zellij", &["list-sessions", "--short", "--no-formatting"]),
        SessionBackend::Tmux => ("tmux", &["ls"]),
    };
    let output = match Command::new(prog).args(args).output() {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        _ => return Vec::new(),
    };
    match backend {
        SessionBackend::Zellij => parse_zellij_ls(&output),
        SessionBackend::Tmux => parse_tmux_ls(&output),
        SessionBackend::Shell => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mux(zellij_layout: &str, zellij_config: &str, tmux_conf: &str) -> MuxConfig {
        MuxConfig {
            zellij_layout: zellij_layout.to_string(),
            zellij_config: zellij_config.to_string(),
            tmux_conf: tmux_conf.to_string(),
        }
    }

    #[test]
    fn shell_backend_uses_shell_with_no_args() {
        let (prog, args) = build_launch_command(
            SessionBackend::Shell,
            "powershell.exe",
            "work",
            &mux("L.kdl", "C.kdl", "t.conf"),
        );
        assert_eq!(prog, "powershell.exe");
        assert!(args.is_empty());
    }

    #[test]
    fn zellij_backend_attach_or_create_with_config_and_layout() {
        // PoC 確定形 + Den config: zellij --config <cfg> -l <layout> attach -c <name>
        let (prog, args) = build_launch_command(
            SessionBackend::Zellij,
            "powershell.exe",
            "work",
            &mux("L.kdl", "C.kdl", "t.conf"),
        );
        assert_eq!(prog, "zellij");
        assert_eq!(
            args,
            vec!["--config", "C.kdl", "-l", "L.kdl", "attach", "-c", "work"]
        );
    }

    #[test]
    fn tmux_backend_attach_or_create_with_conf() {
        let (prog, args) = build_launch_command(
            SessionBackend::Tmux,
            "powershell.exe",
            "work",
            &mux("L.kdl", "C.kdl", "t.conf"),
        );
        assert_eq!(prog, "tmux");
        assert_eq!(
            args,
            vec!["-f", "t.conf", "new-session", "-A", "-s", "work"]
        );
    }

    #[test]
    fn zellij_backend_without_config_or_layout_omits_flags() {
        // config/layout 書き出し失敗（空文字列）→ --config も -l も付けない
        let (prog, args) = build_launch_command(
            SessionBackend::Zellij,
            "powershell.exe",
            "work",
            &mux("", "", ""),
        );
        assert_eq!(prog, "zellij");
        assert_eq!(args, vec!["attach", "-c", "work"]);
    }

    #[test]
    fn zellij_backend_config_without_layout() {
        // config だけ成功・layout 失敗 → --config のみ付与（-l は省略）
        let (_, args) = build_launch_command(
            SessionBackend::Zellij,
            "powershell.exe",
            "work",
            &mux("", "C.kdl", ""),
        );
        assert_eq!(args, vec!["--config", "C.kdl", "attach", "-c", "work"]);
    }

    #[test]
    fn tmux_backend_without_conf_omits_flag() {
        let (prog, args) = build_launch_command(
            SessionBackend::Tmux,
            "powershell.exe",
            "work",
            &mux("", "", ""),
        );
        assert_eq!(prog, "tmux");
        assert_eq!(args, vec!["new-session", "-A", "-s", "work"]);
    }

    #[test]
    fn hyphenated_name_is_not_confused_with_flag() {
        // name が "-l"/"-f" と一致しても layout 省略時に壊れない（文字列除去ではなく条件付き付与）
        let (_, zargs) = build_launch_command(
            SessionBackend::Zellij,
            "powershell.exe",
            "-l",
            &mux("", "", ""),
        );
        assert_eq!(zargs, vec!["attach", "-c", "-l"]);
        let (_, targs) = build_launch_command(
            SessionBackend::Tmux,
            "powershell.exe",
            "-f",
            &mux("", "", ""),
        );
        assert_eq!(targs, vec!["new-session", "-A", "-s", "-f"]);
    }

    #[test]
    fn backend_default_is_shell() {
        assert_eq!(SessionBackend::default(), SessionBackend::Shell);
    }

    #[test]
    fn parse_zellij_ls_short_bare_names() {
        // PoC 確定: --short --no-formatting は名前のみ
        let out = "main\nwork\n";
        assert_eq!(parse_zellij_ls(out), vec!["main", "work"]);
    }

    #[test]
    fn parse_zellij_ls_strips_decoration_defensively() {
        // 装飾付き（--short 無し）でも行頭トークンを拾い ANSI を落とす
        let out = "\u{1b}[32;1mmain\u{1b}[m [Created 1h ago]\nwork [Created 2m ago] (EXITED)\n";
        assert_eq!(parse_zellij_ls(out), vec!["main", "work"]);
    }

    #[test]
    fn parse_tmux_ls_extracts_names() {
        let out = "main: 1 windows (created Sat) [80x24]\nagent: 2 windows (created Sat)\n";
        assert_eq!(parse_tmux_ls(out), vec!["main", "agent"]);
    }

    #[test]
    fn parse_handles_empty() {
        assert!(parse_zellij_ls("").is_empty());
        assert!(parse_tmux_ls("").is_empty());
    }
}
