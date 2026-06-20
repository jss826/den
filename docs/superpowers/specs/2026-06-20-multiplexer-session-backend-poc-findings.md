# Multiplexer Session Backend — PoC Findings

> Task 1（実機 PoC）の確定値。Task 2 以降の argv / layout / パーサ / detach はこの文書を正とする。
> 計測環境: Windows 11 Pro (10.0.26200) / zellij 0.44.3 / tmux=未導入。計測日 2026-06-20。

## サマリ（plan からの逸脱）

| 項目 | plan の想定 | PoC 確定値 |
|---|---|---|
| zellij 起動 argv | `zellij --session <name> --layout <path>` | **`zellij -l <path> attach -c <name>`** |
| zellij ls パース | `list-sessions --no-formatting` + 行頭トークン抽出 | **`list-sessions --short --no-formatting`**（名前のみ・1行1名） |
| zellij sanitize | `--layout <path>` ペア除去 | **`-l <path>` ペア除去**（フラグ名が `-l`） |
| detach=生存 | 想定 Yes | **Yes（実証）。ただし Job Object 注意あり（後述）** |
| tmux | 実機 PoC | **Windows 未導入 → argv は標準形で確定、empirical は Task 14 remote スモークへ** |

---

## 1. zellij 起動コマンド（attach-or-create + bare layout）

**確定形:**

```
zellij -l <layout_path> attach -c <name>
```

argv 配列: `["zellij", "-l", <layout_path>, "attach", "-c", <name>]`

**実証:**
- 初回実行 → セッション作成 + server プロセス spawn。
- 既存名で再実行 → **attach（ブロック＝合流）、error にならない**。→ Den 再起動後の再 spawn でそのまま合流できる。

**plan の `zellij --session <name> --layout <path>` は誤り:**
既存セッションがあると即 error で終了する:

```
Session with name "<name>" already exists. Use attach command to connect to it or specify a different name.
```

`-s/--session` は「新規セッション専用」。Den は server 生存跨ぎで再 spawn するため attach-or-create が必須 → `attach -c` を使う。`-l`（global layout）は **初回 create 時のみ**適用され、attach 時は無視される（既存セッションのレイアウトを維持）。

**Task 2 への反映:** `build_launch_command` の Zellij 分岐は `("zellij", vec!["-l", zellij_layout, "attach", "-c", name])` を返す。テストの期待値もこれに更新。

---

## 2. ls パース

**確定形:**

```
zellij list-sessions --short --no-formatting
```

→ セッション名のみ、1行1名（`[Created ...]` / `(current)` / `(EXITED - attach to resurrect)` の装飾接尾辞なし）。

実測:
- `zellij ls`（装飾）: `\x1b[32;1mden-poc\x1b[m [Created \x1b[35;1m2m 14s\x1b[m ago] `（ANSI + 接尾辞）
- `zellij ls --no-formatting`: `den-poc [Created 2m 14s ago] `（ANSI なし・接尾辞あり）
- `zellij ls --short --no-formatting`: `den-poc`（**名前のみ**）

**空のとき:** stderr に `No active zellij sessions found.`、**exit code 1**。→ `list_mux_sessions` は `status.success()==false` で空 Vec を返すので問題なし。

**Task 7 への反映:** zellij の list argv を `["list-sessions", "--short", "--no-formatting"]` に。パーサは「行 trim → 空行除外」で十分（plan の whitespace split 先頭トークンでも可）。ANSI strip は防御的に残してよいが `--no-formatting` で原則不要。

---

## 3. availability probe

`zellij --version` → `zellij 0.44.3`、exit 0。`probe_available(Zellij)` はこのままで可。

---

## 4. detach = クリーン / セッション生存（核心）

**結論: Yes（実証）。** Den が spawn した client を kill しても zellij server は生存しセッションは残る。

**プロセス構造（本物の console を与えた場合）:**
- client: `zellij -l <layout> attach -c <name>`
- server: `zellij --server <...\Temp\zellij\contract_version_1\<name>>`（client の**子プロセス**）

**実測:**
- client のみを `Stop-Process` → **server 生存・`zellij ls` に名前残存** ✅

### ⚠️ 実装上の重大注意（Task 3/6 + Task 14 で要検証）

1. **Job Object 道連れリスク**: zellij server は client の**子**として生成される。Den の PTY（portable-pty）は Windows **Job Object** に client を割り当てる。Job が `KILL_ON_JOB_CLOSE` を持ち、子（server）も同 Job に入る場合、**client kill = server も道連れ**でセッションが死ぬ恐れ。
   - 私の Start-Process 単体テスト（Job 無し）では server 生存を確認。
   - **Den の Job Object 文脈では未確認。** Task 3/6 実装時に「server が Den の Job から breakaway するか」を確認し、必要なら `CREATE_BREAKAWAY_FROM_JOB` 相当の対処、または mux backend では kill-on-close を外す。Task 14 スモークで「Den destroy → 別端末から同名 attach で生存」を必ず実機確認。
2. **本物の console（TTY）が必須**: console なし（`Start-Process -WindowStyle Hidden` 等の非 TTY）では zellij が server を分離せず、kill でセッションが死んだ。Den は portable-pty で本物の ConPTY を与えるため「console あり = server 分離」ケースに一致する想定。

---

## 5. bare layout

**確定ファイル `den-bare.kdl`:**

```kdl
layout {
    pane
}
```

`zellij setup --dump-layout default` は tab-bar / status-bar plugin pane を含む:

```kdl
layout {
    pane size=1 borderless=true { plugin location="tab-bar" }
    pane
    pane size=1 borderless=true { plugin location="status-bar" }
}
```

→ これらの plugin pane を省いた `layout { pane }` が bare。**構造的に確定**。視覚的な bar 非表示確認は対話 attach が要るため Task 14 スモークに回す（未 attach セッションの `dump-screen` は空で判定不可だった）。

---

## 6. tmux（Windows 未導入 → 標準形で確定、empirical は後送り）

- **この Windows ホストに tmux は無い**（Windows ネイティブ非対応・WSL のみ。memory 既知）。実機 PoC 不可。
- **argv（標準形・確定）:** `tmux -f <conf> new-session -A -s <name>`（`-A` = attach-or-create）。
- **conf `den.conf`:**
  ```
  set -g status off
  set -g window-size latest
  ```
- **ls パース:** `tmux ls` → `name: N windows (created ...) ...` の `:` 前がセッション名。plan の `parse_tmux_ls` のままで可。
- **empirical 確認 = Task 14 の remote nix-box スモーク**（別ホストの tmux に Quick Connect）。

---

## 7. clamp / latest-active

- **tmux:** `set -g window-size latest`（直近操作クライアントにサイズ追従）。標準・確定。
- **zellij:** 異サイズの実クライアント2台（PC + iPad）同時 attach が要るため非対話・単一ホストでは未計測。zellij は歴史的に「最小クライアントへ clamp（per-client リサイズなし）」。**Task 14 iPad スモークで実測 → spec の TBD #2 を確定値に更新。**

---

## 8. その他の便利知見

- `zellij --session <name> action <subcmd>`（例 `dump-screen`）は**既存セッションをターゲットできる**（bare `zellij --session <name>` が error なのと対照的）。将来の制御に有用。
- `zellij attach -b/--create-background` は非対話でデタッチ作成できるが、本物の console を伴わないと server 分離しない場合がある（上記 §4 注意2）。

---

## 後続タスクへの確定反映チェックリスト

- [ ] Task 2: zellij argv = `["-l", layout, "attach", "-c", name]`、テスト期待値更新。tmux argv は標準形維持。
- [ ] Task 4: `den-bare.kdl` = `layout { pane }`、`den.conf` = status off + window-size latest。
- [ ] Task 6: `sanitize_missing_layout` の zellij は **`-l` ペア**除去（`--layout` ではない）。
- [ ] Task 7: zellij list argv = `["list-sessions", "--short", "--no-formatting"]`、パーサは行 trim。
- [ ] Task 3/6/14: Job Object × zellij server breakaway を検証（detach 生存の実機確認）。
- [ ] Task 14: bars 非表示 visual / tmux 実機 / clamp（iPad）/ remote tmux スモーク。
