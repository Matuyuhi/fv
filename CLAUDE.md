# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## 概要

fv は読み取り専用の TUI コードビューア（ratatui + crossterm + syntect + ignore + notify）。編集機能は意図的に持たない。新規依存の追加は原則しない方針（ファジーマッチ・git 連携は依存を足さず自前実装 / git CLI 呼び出しで済ませている）。

## コマンド

```sh
cargo build            # 警告ゼロを維持する
cargo run -- <dir>     # 起動（dir 省略時はカレント）。日常使いは --release（debug は syntect 初期化で起動に 1-2 秒）
cargo clippy
cargo fmt
```

テストは現状なし。動作確認は pty 経由のスモークテストで行う:

```sh
{ sleep 2.5; printf 'jj'; sleep 0.3; printf '\r'; sleep 0.5; printf 'q'; } | \
  script -q /dev/null sh -c "stty rows 30 cols 100; ./target/debug/fv <dir>" > out.raw 2>&1
LC_ALL=C grep -ao '<marker>' out.raw
```

- 初期 sleep 2.5s 以上（起動前に届いたキーは cooked mode に流れて失われる。capture 先頭に `jj^M` の echo が出たら疑う）
- `stty` でのサイズ付与必須（サイズ 0 だと何も描画されない）
- マウスは SGR シーケンス注入: クリック `\x1b[<0;COL;ROW M` + `\x1b[<0;COL;ROW m`、ホイール下 `\x1b[<65;x;y M`（実際は空白なし）
- **罠**: ratatui は差分描画（前フレームと違うセルだけ出力）のため、コンテンツ文字列の grep は偽陰性を出す。gutter 行番号で判定するか、ファイル切替を挟んで全面再描画させる

## アーキテクチャ

### イベントループ（main.rs）
`event::poll(100ms)` → Key/Mouse を App へ → 毎 tick `app.on_tick()`（FS 監視の drain）→ 毎ループ再描画。ブロッキング read にしないこと（自動リロードと 100ms 周期再描画がこの構造に依存）。端末復元は `restore_terminal()` に集約され panic hook からも呼ばれる。raw mode / alternate screen / mouse capture の解除を追加・変更する時は必ずここに入れる。

### モジュール構成（1 型 1 責務 1 ファイル方針）
- `app/` — mod.rs(App 状態・on_tick), keys.rs(全キールーティング), mouse.rs, mode.rs(Focus/Mode/InputKind)
- `tree/` — mod.rs(選択・展開操作), node.rs, scan.rs(走査・rescan ヘルパー)
- `viewer/` — mod.rs(open/reload/スクロール/履歴), content.rs(読込・syntect ハイライト), search.rs
- `ui/` — mod.rs(draw・レイアウト), tree_pane.rs, viewer_pane.rs, status_bar.rs, finder_panel.rs, help.rs
- `finder.rs`(ファジーマッチ自前実装) / `git.rs`(git CLI ラッパー) / `watch.rs`(notify)

### キールーティングの優先順位（app/keys.rs on_key）
Ctrl+c → Mode::Help → Mode::Finder → Mode::Input(Search/Goto) → Normal(q/Tab → focus 別ディスパッチ)。新しいモード・キーを足す時はこの順序に組み込む。`pending_g`（gg 待ち）は Tree/Viewer で共用され、Tab・マウスでリセットされる。

### 桁位置の整合インバリアント（複数ファイルに跨る前提）
- 各行 `Line` の **span[0] は行番号 gutter**。検索ハイライト・水平スクロールは span[1..] を char 単位で走査する
- `Content::Text { lines, plain }` の plain は normalize 済み（改行除去・タブ→スペース4）で、**char インデックスが描画桁と 1:1 対応**する。検索マッチの (line, start_col, end_col) はこの前提で ui 側の bg 重ねに直結する
- 大文字小文字の畳み込みは ASCII 限定（`to_ascii_lowercase`）。Unicode の完全 case folding は char 数が変わり桁対応が壊れるため意図的に使っていない（viewer/search.rs と finder.rs の両方）
- viewer_pane の行加工順は `mark_changed_line → highlight_matches → hscroll_line` 固定。hscroll を先にすると検索マッチの絶対桁がズレる
- gutter の変更行マーク `▎` は「gutter 末尾の空白 1 文字を置き換える」方式で char 数を維持している

### 描画は自前スライス
`Paragraph::scroll` は u16 上限で使わない。`lines[scroll..scroll+height]` を毎フレームスライスして描画する。ui は `viewport_height` / `viewport_width` / `tree_area` / `viewer_area` を毎フレーム App/Viewer に書き戻し、キー・マウス処理側がそれを読む（ui→app の逆流はこのパターンに統一）。

### ツリー走査と FS 監視
- 走査は起動時に WalkBuilder 1 回で一括（サブディレクトリ起点の遅延走査だと親の .gitignore が効かない）。`require_git(false)` で非 git ディレクトリでも .gitignore を尊重
- `rescan` は展開状態と選択を **path で**保存・復元する（index_path は再走査で無効になる）
- watch.rs のイベントフィルタは「`.` 始まり成分の除外 + root .gitignore の `matched_path_or_any_parents`」（`matched` だと `target/` が配下パスに効かない）。ツリー再走査は 500ms デバウンスで、git status の再取得もこれに相乗りする（別タイマーを作らない）

### git 連携（git.rs）
git2 クレートは使わず CLI を `GIT_OPTIONAL_LOCKS=0` 付きで実行。porcelain -z の rename は `XY new\0old\0` の 2 パス形式。`git diff HEAD` は HEAD 無し repo で fail するため素の diff にフォールバックする。全失敗を Option で吸収し panic しない。

## スタイル

- コメントは Why のみ・日本語。What の説明やコード写経コメントは書かない
- ハイライトのキャッシュ（`HashMap<PathBuf, Rc<Content>>`）を素通りする描画パスを足さない（再描画毎の再ハイライト禁止）
