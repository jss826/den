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

/// backend に応じた起動コマンド (program, args) を組み立てる。
/// シェル文字列連結はしない（argv 配列で CommandBuilder に渡す）。
/// name は is_valid_session_name で英数＋`-` に限定済みの前提。
///
/// zellij/tmux の argv は Task 1 PoC findings の確定形を使う:
/// - zellij: `zellij -l <layout> attach -c <name>`（attach-or-create。初回 create 時のみ layout 適用）
/// - tmux:   `tmux -f <conf> new-session -A -s <name>`（`-A` で attach-or-create）
///
/// layout/conf パスが空（embed 書き出し失敗）のときは layout フラグを最初から付けず、
/// 素の `zellij attach -c <name>` / `tmux new-session -A -s <name>` を返す。
/// 空文字列を引数として渡したり、後段で文字列一致で除去したりはしない
/// （name が偶然 `-l`/`-f` と一致しても壊れない）。
pub fn build_launch_command(
    backend: SessionBackend,
    shell: &str,
    name: &str,
    zellij_layout: &str,
    tmux_conf: &str,
) -> (String, Vec<String>) {
    match backend {
        SessionBackend::Shell => (shell.to_string(), Vec::new()),
        SessionBackend::Zellij => {
            let mut args = Vec::new();
            if !zellij_layout.is_empty() {
                args.push("-l".to_string());
                args.push(zellij_layout.to_string());
            }
            args.push("attach".to_string());
            args.push("-c".to_string());
            args.push(name.to_string());
            ("zellij".to_string(), args)
        }
        SessionBackend::Tmux => {
            let mut args = Vec::new();
            if !tmux_conf.is_empty() {
                args.push("-f".to_string());
                args.push(tmux_conf.to_string());
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

    #[test]
    fn shell_backend_uses_shell_with_no_args() {
        let (prog, args) = build_launch_command(
            SessionBackend::Shell,
            "powershell.exe",
            "work",
            "L.kdl",
            "t.conf",
        );
        assert_eq!(prog, "powershell.exe");
        assert!(args.is_empty());
    }

    #[test]
    fn zellij_backend_attach_or_create_with_layout() {
        // PoC 確定形: zellij -l <layout> attach -c <name>
        let (prog, args) = build_launch_command(
            SessionBackend::Zellij,
            "powershell.exe",
            "work",
            "L.kdl",
            "t.conf",
        );
        assert_eq!(prog, "zellij");
        assert_eq!(args, vec!["-l", "L.kdl", "attach", "-c", "work"]);
    }

    #[test]
    fn tmux_backend_attach_or_create_with_conf() {
        let (prog, args) = build_launch_command(
            SessionBackend::Tmux,
            "powershell.exe",
            "work",
            "L.kdl",
            "t.conf",
        );
        assert_eq!(prog, "tmux");
        assert_eq!(
            args,
            vec!["-f", "t.conf", "new-session", "-A", "-s", "work"]
        );
    }

    #[test]
    fn zellij_backend_without_layout_omits_flag() {
        // layout 書き出し失敗（空文字列）→ -l フラグごと付けない
        let (prog, args) =
            build_launch_command(SessionBackend::Zellij, "powershell.exe", "work", "", "");
        assert_eq!(prog, "zellij");
        assert_eq!(args, vec!["attach", "-c", "work"]);
    }

    #[test]
    fn tmux_backend_without_conf_omits_flag() {
        let (prog, args) =
            build_launch_command(SessionBackend::Tmux, "powershell.exe", "work", "", "");
        assert_eq!(prog, "tmux");
        assert_eq!(args, vec!["new-session", "-A", "-s", "work"]);
    }

    #[test]
    fn hyphenated_name_is_not_confused_with_flag() {
        // name が "-l"/"-f" と一致しても layout 省略時に壊れない（文字列除去ではなく条件付き付与）
        let (_, zargs) =
            build_launch_command(SessionBackend::Zellij, "powershell.exe", "-l", "", "");
        assert_eq!(zargs, vec!["attach", "-c", "-l"]);
        let (_, targs) = build_launch_command(SessionBackend::Tmux, "powershell.exe", "-f", "", "");
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
