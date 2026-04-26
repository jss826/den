---
name: deps-check
description: Rust crate と frontend npm パッケージのバージョンを確認し、更新推奨を報告する。「依存関係チェック」「ライブラリ更新」「deps update」「outdated」で使用。
---

# deps-check

プロジェクトの依存関係バージョンを確認し、段階的に更新する。

## 目次

- [対象](#対象)
- [実行手順](#実行手順)
- [制約・落とし穴](#制約落とし穴)
- [release との連携](#release-との連携)

## 対象

| カテゴリ | ソース | チェック方法 |
|---------|--------|------------|
| Rust crates | `Cargo.toml` の `[dependencies]` / `[dev-dependencies]` | crates.io / lib.rs の latest stable |
| Frontend npm | `package.json` の `devDependencies`（vendor JS は npm 経由でインストール） | npmjs.com の latest |
| Vendor bundle | `frontend/vendor/` | npm パッケージの bump 後に `npm run build:vendor` で再生成 |

## 実行手順

### 1. 現在のバージョン収集

- **Rust**: `Cargo.toml` を Read。実値は `Cargo.lock` の `name = "..."` + `version = "..."` で確認できる
- **npm**: `package.json` の `devDependencies` を Read

### 2. 最新バージョン確認

`Agent(general-purpose)` に WebFetch / WebSearch を投げて並列調査。Rust と npm は別エージェントに分けて投げると効率が良い。

確認ポイント:
- メジャー / マイナー / パッチの差分種別
- 破壊的変更の有無（changelog / release notes 一行要約）
- セキュリティアドバイザリ
- 既知の互換性制約（`memory/deps-compat.md` 参照）

### 3. レポート生成

3 表形式:

```
### 1. メジャー/マイナー更新あり（要レビュー）
| ライブラリ | 現在 | 最新 | 種別 | 注意 |

### 2. パッチ更新あり（軽微）
| ライブラリ | 現在 | 最新 |

### 3. 既に最新
```

その他に **保留継続（既知制約）** のセクションを `memory/deps-compat.md` から再掲する。

### 4. 段階的更新（ユーザー承認後）

リスクの低い順に **Phase 別に commit** する。1 Phase = 1 commit が原則。

#### Phase A: Rust パッチ + npm caret 内 minor（一括安全）

```bash
# Rust: Cargo.toml を書き換えなくても caret 範囲で取れる
cargo update

# npm: caret 範囲で minor も上がる。--legacy-peer-deps は必須（後述）
npm update --save --legacy-peer-deps
```

Playwright や esbuild が caret 内で大きく上がることがあるので、`git diff package.json` で確認する。

#### Phase B 以降: メジャー / 0.x マイナー（個別承認）

1. `Cargo.toml` または `package.json` のバージョン指定を書き換え
2. Rust なら `cargo build` → API 変更があれば修正、npm なら `npm install --legacy-peer-deps`
3. 必要なら vendor 再ビルド（後述）
4. 品質ゲート → commit

### 5. 品質ゲート（毎 Phase 後）

```bash
cargo fmt -- --check
cargo clippy -- -D warnings
cargo test --target-dir target-test
npm run lint
npm test
```

UI レンダラ系（@wterm / @xterm / esbuild）を触ったら、最後にまとめて E2E:

```bash
npx playwright test
```

## 制約・落とし穴

### npm peer dep（必須）

- `@xterm/addon-canvas@0.7.0` の peer は `@xterm/xterm: ^5.0.0` ピンだが当方は v6 → **`npm install` / `npm update` は常に `--legacy-peer-deps`** が必要
- `.npmrc` には未保存。一括コマンドのたびに明示する

### @wterm publish バグ

- `@wterm/dom@0.2.0` の published manifest に `"@wterm/core": "workspace:*"` が残ったまま → `EUNSUPPORTEDPROTOCOL` で install 失敗
- 0.1.9 までは正常。**0.2.x は upstream 修正待ちで blocked**
- 詳細: `memory/deps-compat.md`

### vendor bundle 再生成

| トリガー | コマンド | bump 必要 |
|---|---|---|
| `@wterm/core` or `@wterm/dom` 更新 | `npm run build:wterm` | `WTERM_VERSION` (in `wterm-xterm-adapter.js`) + `?v=N` (in `frontend/js/terminal-adapter.js`) を同じ値で bump |
| `esbuild` 更新 | `npm run build:vendor` | bundle 出力が変わるだけなので bump 不要、ただし git diff で確認 |
| `codemirror` / `@codemirror/lang-*` 更新 | `npm run build:codemirror` | 上記と同じ |

### Playwright major bump

- 1.50→1.59 のような major bump 後は **chromium バイナリのキャッシュが無効化** される
- `Executable doesn't exist at ...chrome-headless-shell.exe` で E2E が全滅する
- **対処**: `npx playwright install chromium`（111 MiB DL）

### ESLint major bump

- 9→10 のような major bump で `recommended` ruleset に新しいルールが追加されることがある
- 例: ESLint 10 で `no-useless-assignment` が recommended に昇格 → 既存の dead initializer が error になる
- 対処: `npm run lint` で error 行を 1 つずつ修正

### RustCrypto digest 系

- `sha2 0.x` / `hmac 0.x` の major bump で trait import 経路が変わることがある
- 例: hmac 0.13 で `new_from_slice` が `KeyInit` trait 経由必須に → `use hmac::KeyInit;` の追加が必要
- 詳細: `memory/deps-compat.md`

### 保留中の Rust 制約

`memory/deps-compat.md` 参照。代表例:

| crate | 保留理由 |
|---|---|
| `rand 0.8` | `russh 0.57` が `rand_core 0.6` 依存 |
| `russh 0.57` | rand 0.9 化未対応 |
| `windows-sys 0.59` | `portable-pty 0.9` 互換性 |
| `aes-gcm 0.10` | 0.11 は RC のみ |

## release との連携

依存関係更新だけの session でも、最後に `/release` で patch bump（例: v3.3.0 → v3.3.1）を切るとバイナリが配布される。リリースノートのカテゴリは **Other** で `Bump <pkg> <old> → <new>` 形式に揃える。
