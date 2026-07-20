# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## 概要

fv は TUI コードビューア + インライン編集（ratatui + crossterm + syntect + ignore + notify）。当初は読み取り専用方針だったが、「AI が書いたコードをその場で手直しする」用途のため編集機能を段階導入中（Stage 1: 挿入・削除・undo/redo・ペースト・保存 済み / 将来: 選択・yank、vim 風モーダル）。VSCode 級の完全なエディタは目指さない。新規依存の追加は原則しない方針（ファジーマッチ・git 連携・編集バッファは依存を足さず自前実装 / git CLI 呼び出しで済ませている）。

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
`event::poll(100ms)` → Key/Mouse/Paste を App へ → 毎 tick `app.on_tick()`（FS 監視の drain）→ 毎ループ再描画。ブロッキング read にしないこと（自動リロードと 100ms 周期再描画がこの構造に依存）。端末復元は `restore_terminal()` に集約され panic hook からも呼ばれる。raw mode / alternate screen / mouse capture / bracketed paste / keyboard enhancement の解除を追加・変更する時は必ずここに入れる。
- kitty keyboard protocol を対応端末（ghostty/kitty/WezTerm 等）で opt-in している。有効時はキー長押しが `KeyEventKind::Repeat` で届くため、イベントフィルタは「Release 以外」で受ける（Press 限定に戻すと長押しリピートが死ぬ）。mac の Cmd は SUPER 修飾として届く（未対応端末では届かない = Cmd バインドは補助扱いに留める）
- 修飾付き文字キーは端末により大文字で届くことがある。修飾キーバインドのマッチは `to_ascii_lowercase` で畳んでから行う（editor/mod.rs handle_key 参照）

### モジュール構成（1 型 1 責務 1 ファイル方針）
- `app/` — mod.rs(App 状態・on_tick), keys.rs(全キールーティング), mouse.rs, mode.rs(Focus/Mode/InputKind)
- `tree/` — mod.rs(選択・展開操作), node.rs, scan.rs(走査・rescan ヘルパー)
- `viewer/` — mod.rs(open/reload/履歴・cache), viewport.rs(Viewport: スクロール・折返し状態), highlight.rs(Highlighter: syntect・テーマ), content.rs(読込・Content/Open), search.rs
- `editor/` — mod.rs(EditState: カーソル・キー処理・追従), buffer.rs(EditBuffer: 生テキスト・undo/redo)
- `ui/` — mod.rs(draw・レイアウト), tree_pane.rs, text_pane.rs(閲覧・編集共通の描画コア), viewer_pane.rs, editor_pane.rs, status_bar.rs, finder_panel.rs, help.rs
- `text.rs`(タブ幅・gutter 幅・桁変換の唯一の定義) / `finder.rs`(ファジーマッチ自前実装) / `git.rs`(git CLI ラッパー) / `watch.rs`(notify)

### 閲覧と編集の関係（後付けにしない）
- `Viewport`（scroll/hscroll/wrap/実測サイズ）は閲覧・編集で**同じ実体を共有**する。モード遷移で位置が飛ばない根拠はここ。「wrap 中は hscroll = 0」のインバリアントは Viewport のメソッドと EditState::ensure_visible が守る（モード出口での手当てはしない）
- `Highlighter` は syntect 一式の置き場。EditState は Viewer 全体ではなく **Highlighter と Viewport だけを借りる**（保存だけは cache 即時更新のため `Viewer::reload` を呼ぶ）。editor に新しい操作を足す時もこの依存範囲を広げない
- 描画は `ui/text_pane.rs` の `TextPane` に一本化（閲覧 = search あり cursor なし / 編集 = cursor あり search なし）。行加工順は `mark_changed_line → highlight_matches → (hscroll | char 単位 wrap) → cursor overlay` 固定
- wrap は閲覧・編集とも **char 単位の自前分割**（`Paragraph::wrap` は単語境界 wrap で折返し位置が外から計算できないため全面的に不使用）。視覚行数は描画（text_pane）・カーソル追従（ensure_visible）・クリック座標（click_at）の 3 者が `text::wrap_rows` を共有し、ズレると即カーソル位置バグになる

### キールーティングの優先順位（app/keys.rs on_key）
Ctrl+c → Mode::Help → Mode::Settings → Mode::Finder → Mode::Input(Search/Goto) → Mode::Edit → Normal(q/Tab → focus 別ディスパッチ)。新しいモード・キーを足す時はこの順序に組み込む。Edit はグローバルキー（q/s/Tab/Ctrl+p）より前に置くことで印字キーを全て文字入力にしている（Ctrl+c だけは強制終了として残る）。`pending_g`（gg 待ち）は Tree/Viewer で共用され、Tab・マウスでリセットされる。

### 桁位置の整合インバリアント（複数ファイルに跨る前提）
- 各行 `Line` の **span[0] は行番号 gutter**。検索ハイライト・水平スクロールは span[1..] を char 単位で走査する
- `Content::Text { lines, plain }` の plain は normalize 済み（改行除去・タブ→スペース4）で、**char インデックスが描画桁と 1:1 対応**する。検索マッチの (line, start_col, end_col) はこの前提で ui 側の bg 重ねに直結する
- タブ幅・gutter 幅・表示桁⇔char 座標の換算は **`text.rs` が唯一の定義**。閲覧（content.rs の normalize）と編集（カーソル・クリック座標）が別々に持つとここが最初に壊れる
- 大文字小文字の畳み込みは ASCII 限定（`to_ascii_lowercase`）。Unicode の完全 case folding は char 数が変わり桁対応が壊れるため意図的に使っていない（viewer/search.rs と finder.rs の両方）
- text_pane の行加工順は `mark_changed_line → highlight_matches → hscroll_line` 固定。hscroll を先にすると検索マッチの絶対桁がズレる
- gutter の変更行マーク `▎` は「gutter 末尾の空白 1 文字を置き換える」方式で char 数を維持している

### 描画は自前スライス
`Paragraph::scroll` は u16 上限で使わない。`lines[scroll..scroll+height]` を毎フレームスライスして描画する（text_pane）。ui は `viewport.height` / `viewport.width` / `tree_area` / `viewer_area` を毎フレーム App/Viewport に書き戻し、キー・マウス処理側がそれを読む（ui→app の逆流はこのパターンに統一）。

### ツリー走査と FS 監視
- 走査は起動時に WalkBuilder 1 回で一括（サブディレクトリ起点の遅延走査だと親の .gitignore が効かない）。`require_git(false)` で非 git ディレクトリでも .gitignore を尊重
- `rescan` は展開状態と選択を **path で**保存・復元する（index_path は再走査で無効になる）
- watch.rs のイベントフィルタは「`.` 始まり成分の除外 + root .gitignore の `matched_path_or_any_parents`」（`matched` だと `target/` が配下パスに効かない）。ツリー再走査は 500ms デバウンスで、git status の再取得もこれに相乗りする（別タイマーを作らない）

### インライン編集（editor/ + ui/editor_pane.rs）
- `Mode::Edit(EditState)` が編集状態（バッファ・カーソル・undo）を所有し、「編集中なのに状態が無い」を型で排除する（Finder と同じパターン）
- `EditBuffer` は disk から**生テキストを独立ロード**する。viewer の `plain` はタブ展開済みで保存に使えない。CRLF・末尾改行を記憶し `to_text()` で復元（保存でファイルを壊さないための核）。undo/redo は Insert/Delete 2 種の op の逆適用で、連続タイピングは coalesce（カーソル移動・改行・保存・ペーストで区切る）
- カーソルは端末カーソルでなく REVERSED スタイル重ね（全角・タブの画面幅計算を回避）。検索ハイライトと同時には使わない（TextPane の search と cursor は排他）
- 編集の度に `Highlighter::highlight_text` で全再ハイライト（256KB 超はプレーン行に切替）。キー入力起因の 1 回きりの再生成であり「再描画毎の再ハイライト禁止」には反しない。`Content` cache は編集中は使わず、保存時の `viewer.reload()` で更新する
- `display_text()` は常に末尾 \n 付き（LinesWithEndings の行数を `lines.len()` に一致させ、末尾空行の描画欠けを防ぐ）
- 変更行マーク `▎` は編集中も出る。ただし viewer と違い**未保存バッファのライブ diff**: 編集開始時に `git.rs::baseline_lines`（HEAD → 初期 repo は index。changed_lines と同じ基準）を 1 回取得し、以後は編集の度に editor/diff.rs（prefix/suffix トリム + LCS 自前実装）で再計算する。git CLI をキーストローク毎に呼ばない
- 既知の制約: 外部変更との競合は last-write-wins（保存が上書きする）。非 UTF-8・10MB 超は編集不可（`e` が no-op）

### git 連携（git.rs）
git2 クレートは使わず CLI を `GIT_OPTIONAL_LOCKS=0` 付きで実行。porcelain -z の rename は `XY new\0old\0` の 2 パス形式。`git diff HEAD` は HEAD 無し repo で fail するため素の diff にフォールバックする。全失敗を Option で吸収し panic しない。

## スタイル

- コメントは Why のみ・日本語。What の説明やコード写経コメントは書かない
- ハイライトのキャッシュ（`HashMap<PathBuf, Rc<Content>>`）を素通りする描画パスを足さない（再描画毎の再ハイライト禁止）
