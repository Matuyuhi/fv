# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## 概要

fv は TUI コードビューア + インライン編集 + 変更レビュー（ratatui + crossterm + syntect + ignore + notify）。当初は読み取り専用方針だったが、「AI が書いたコードをその場で手直しする」用途のため編集機能を段階導入中（Stage 1: 挿入・削除・undo/redo・ペースト・保存 済み / 将来: 選択・yank、vim 風モーダル）。git は GIT レーン（変更ファイル絞り込み + diff 閲覧）まで実装済みで、stage/commit 等の書き込み系は未実装。VSCode 級の完全なエディタは目指さない。新規依存の追加は原則しない方針（ファジーマッチ・git 連携・編集バッファは依存を足さず自前実装 / git CLI 呼び出しで済ませている）。

## コマンド

```sh
cargo build            # 警告ゼロを維持する
cargo run -- <dir>     # 起動（dir 省略時はカレント）。日常使いは --release（debug は syntect 初期化で起動に 1-2 秒）
cargo clippy
cargo fmt
```

見た目の確認は TUI を起動せず**静的プレビュー**で回せる（Compose の `@Preview` / SwiftUI Preview 相当。詳細は「UI プレビュー」節）:

```sh
cargo preview                       # シーン一覧 (= cargo run --features preview -- --preview)
cargo preview git log               # 複数シーンを縦に並べて描き出す
cargo preview all --size 140x40
scripts/preview-watch.sh git        # 保存のたびに再ビルド + 再描画
cargo preview --update-snapshots    # docs/preview/*.svg を焼き直す (UI を変えたらコミットする)
cargo preview view --update-snapshots  # 1 シーンだけ焼き直す
```

速度は `cargo perf`（同じ dev 専用 feature の別入口）で測る。TUI を起動せず、1 打鍵ぶんの「キー入力 → 再描画」の所要時間を TSV で出す（「速度チェック」節）:

```sh
cargo perf                          # = cargo run --release --features preview -- --perf
```

プレビューは **dev 専用の feature**（既定 off）。製品ビルドにはシーン定義も合成リポジトリ生成も入らず、`--preview` 自体が unknown option になる。

UI の回帰は上の**スクリーンショットテスト**（`docs/preview/*.svg` を CI が描き直して突き合わせる）で見る。動作確認は pty 経由のスモークテストで行う:

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
- 修飾付き文字キーは端末により大文字で届くことがある。修飾キーバインドのマッチは `to_ascii_lowercase` で畳んでから行う（component/editor/mod.rs handle_key 参照）

### モジュール構成（コンポーネント単位 + 1 型 1 責務 1 ファイル）
**画面上の 1 つの部品 = 1 フォルダ**で、その状態（mod.rs 以下）と描画（view.rs）を同じ場所に置く。レイヤ別（全ての UI を `component/*/view.rs` に集める）ではなくコンポーネント別にしてあるのは、「issues タブを直す」ときに触る場所を 1 フォルダに閉じるため。ただし**キーの割り当てだけは `app/` に集約したまま**にしてある（下記）。

- `app/` — 合成ルート。全ての状態を所有し、レーン/タブ遷移とキールーティングの優先順位を持つ。mod.rs(App 状態・on_tick・レーン/ワークスペース遷移・rescan/notice), keys.rs(キールーティングの優先順位とレーン/オーバーレイのキー処理), commit.rs(Mode::Commit の開閉・編集・実行), git_ops.rs(stage/discard/stash・fetch/pull/push の実行と後始末), branch_ops.rs(Mode::Branch のキー処理と切替/作成), github_keys.rs(Issues/PullRequests タブのキー処理と gh ジョブ起動), mouse.rs, mode.rs(Focus/Lane/Mode/Workspace/InputKind/ConfirmAction)
  - keys.rs は**「どのキーを誰に渡すか」だけ**を持ち、操作の中身は上記 4 ファイルへ置く。keys.rs が肥大化して優先順位が読めなくなるのを避けるための分割なので、新しい操作を足す時もこの境界を守る（キーの追加は keys.rs、実行の中身は用途別ファイル）。モジュールを跨いで呼ぶメソッドだけ `pub(super)` にする
  - **キー処理をコンポーネント側へ移さないのはなぜか**: ハンドラはほぼ全て「複数のコンポーネントを跨いで App を書き換える」（GIT のキーが `App::rescan` を呼ぶ、issues のキーが `job::spawn` する等）。component 側へ持っていくと component → app の逆向き依存が生まれる。コンポーネント内で閉じる操作は既にその状態型のメソッド（`GitState::next_hunk` / `PrsState::set_open` 等）になっており、keys.rs はそれを呼ぶルータに徹している
  - 書き込み系操作の後の即時再取得は `App::rescan_now`（rescan + デバウンスのタイマー/保留フラグのリセット）に集約する。呼び出し側で 4 行を複製しない
- `component/` — 状態 + その状態だけを受け取る View。各フォルダは `mod.rs`（状態）と `view.rs`（描画）を持つ
  - `tree/` — mod.rs(選択・展開操作), node.rs, scan.rs(1 階層走査・遅延ロード・rescan ヘルパー), view.rs
  - `viewer/` — mod.rs(open/reload/履歴・cache), viewport.rs(Viewport: スクロール・折返し状態), highlight.rs(Highlighter: syntect 一式とテーマ + 行単位で再開できる Session/LineState), render.rs(HighlightCache: 可視範囲だけを組み立てる遅延ハイライト), content.rs(読込・Content/TextDoc/Open), search.rs, selection.rs(Selection: 範囲選択とコピー用のテキスト取り出し), view.rs
  - `editor/` — mod.rs(EditState: カーソル・キー処理・追従), buffer.rs(EditBuffer: 生テキスト・undo/redo), diff.rs(prefix/suffix トリム + LCS。行単位のライブ diff と gitlane の word-level diff が共有する `pub(crate)`), view.rs
  - `gitlane/` — GIT レーンの diff 表示状態。mod.rs が GitState (今どの diff をどう見ているか) と定数/Kind/各 *Diff 構造体、render.rs が inline の行組み立て (render_inline / LOG レーン・PR タブと共有する render_commit)、side.rs が side-by-side (#30)、word.rs が word-level 差分の範囲計算 (#29)、view.rs が描画
  - `log/` — LOG レーンのコミット一覧・ページング・選択 diff の状態 + view.rs(コミット一覧 + diff)
  - `issues/` — issues タブの一覧フィルタ・詳細キャッシュ・ジョブ管理 + view.rs(一覧 + 詳細)
  - `prs/` — pull requests タブの一覧フィルタ・説明/diff/CI 3 種のキャッシュ・ジョブ管理 + view.rs(一覧 + 説明/diff/CI)
  - `remotelist/` — issues/PR が共有する一覧フィルタ (`filter_rows`) と詳細の非同期キャッシュ (`DetailSlot`) + view.rs(一覧・プレーンテキスト詳細の描画部品)
  - `branch/` — BranchState(ブランチ一覧オーバーレイの絞り込み・選択状態) + view.rs
  - `finder/` — mod.rs(ファジーマッチ自前実装), index.rs(FileIndex: Finder 候補の背景全走査), view.rs
- `shell/` — 画面全体の骨格と、App 全体を横断して見せる画面。mod.rs(draw・レイアウト・各 View への値の取り出し), status_bar.rs, tab_bar.rs(Workspace タブバー), help.rs, settings.rs, confirm.rs(確認オーバーレイ), commit.rs(コミットメッセージ入力オーバーレイ)。ここに置くか component に置くかの境界は「専用の状態型を持つか」（「描画の依存範囲」節）
- `widget/` — 複数のコンポーネントが使う描画部品。text_pane.rs(閲覧・編集・diff 共通の描画コア), diff_boundary.rs(sticky header の帯), icons.rs, mod.rs(pane_block / centered_rect)。**どの状態を描くかは持たせない**（渡された Line 列をどう見せるかだけ）
- `preview/` — mod.rs(`--preview` の入口・TestBackend への 1 フレーム描画), scene.rs(シーン定義＝プレビューしたい状態の一覧), keys.rs(シーンを組み立てるキー列 DSL), render.rs(Buffer → ANSI 文字列。手元で見る stdout 用), svg.rs(Buffer → SVG。スナップショット兼 README の画面写真), snapshot.rs(マスクとファイル書き出し), fixture.rs(固定サンプルリポジトリ)。開発用の入口で、アプリ本体からは呼ばれない（「UI プレビュー」節）
- インフラ（どのコンポーネントにも属さない）: `text.rs`(タブ幅・gutter 幅・桁変換の唯一の定義) / `clipboard.rs`(クリップボードへの書き出し。外部コマンド → OSC 52 のフォールバックと自前 base64) / `git/`(git CLI ラッパー。mod.rs が実行レイヤ (run_git / run_git_write と出力整形) と全再エクスポート、status.rs(porcelain パース)・diff.rs(changed_lines/baseline_lines/file_diff/diff_all/truncate_diff)・log.rs・write.rs(stage/unstage/discard/commit)・component/branch/mod.rs(branches/branch_status/switch 系)・remote.rs(fetch/pull/push) にコマンドを分ける。呼び出し側から見えるパスは分割前と同じ `git::foo`) / `github.rs`(GitHub モードが使えるか 1 箇所で判定する check_available に加え、gh CLI ラッパー: issues/PR 一覧・詳細取得の `list_issues`/`issue_detail`/`open_issue_web`/`list_prs`/`pr_detail`/`pr_diff`/`pr_checks`/`open_pr_web`) / `job.rs`(非同期ジョブの基盤。thread::spawn + mpsc::channel の薄いラッパー) / `watch.rs`(notify) / `config.rs`
- **可視性**: component/widget は別のモジュールツリーから呼ばれるので、跨いで使うものは `pub(crate)` になる（レイヤ別構成なら `component/*/view.rs` 内で `pub(super)` に閉じられていた分の代償）。フォルダ内に閉じるものは `pub(super)` のままにする

### Workspace（タブ）・レーン（Lane）・オーバーレイ（Mode）の3軸
キーマップ飽和を避けるため、状態を3軸に分けている。**新しい機能を足す時はどの軸かをまず決める**。
- `Workspace`（app/mode.rs）= トップレベルのタブ。`Viewer` / `Issues` / `PullRequests` の3つで、GitHub モード（既定 off）有効時だけ **Ctrl+t で循環**（`App::cycle_workspace`）・Alt+1..3 で直接指定・タブクリックで切替。`Workspace::Viewer` が既存アプリ全体（Lane 3 種 + ツリー + オーバーレイ）にあたり、Issues/PullRequests は「ローカルのファイル」という文脈を共有しないリモートのデータなので Lane には混ぜない（Shift+Tab で編集中から PR 一覧に飛ぶとレーンの意味が壊れるため）。GitHub モードが無効/使えない間は Workspace は Viewer 固定で、タブバーの1行も確保しない（`shell::draw` が `App::workspace_available` 1 箇所で判定）
- `Lane`（app/mode.rs）= Viewer タブの中の持続する作業レーン。`View` / `Edit(EditState)` / `Git(GitState)` / `Log(LogState)` の4つで、**Shift+Tab で循環**（`App::cycle_lane`）。Edit・Git・Log は自分の状態を所有し「そのレーンにいるのに状態が無い」を型で排除する
- `Mode` = レーンの上に重なる一時オーバーレイ（Input/Finder/Help/Settings）。閉じると `Mode::Normal` に戻るが**レーンは変わらない**（GIT でヘルプを開いて閉じても GIT に戻る）。この分離のために `Mode::Edit` を `Lane::Edit` へ移した経緯がある。Workspace を跨いでも同様にモードは独立している
- 入れないレーンは循環時にスキップする（非テキスト → EDIT、非 git repo → GIT/LOG）。判定は `enter_edit` / `enter_git` / `enter_log` が false を返す形に閉じ込め、呼び出し側で条件を二重に書かない。GIT (`git_available`: 変更が1件以上) と LOG (`log_available`: git repo でありさえすればよい) は判定基準が違う点に注意（LOG はコミット 0 件の repo でも一覧を出して「no commits」を見せるだけで良いため）
- **Shift+Tab は Edit レーンより前に処理する**（keys.rs）。印字キーではないので「編集中は印字キーを全て文字入力にする」ポリシーとは衝突しない。ただし未保存バッファがある間はレーンを変えず notice を出す。Issues/PR タブに Lane の概念は無いので、そこに居る間 `cycle_lane` 自体が no-op になる（ステータスバーのレーンセグメントも合わせて暗くする）
- `Focus`（Tree/Viewer）はレーンと直交する。GIT・LOG でも Tab で左右を行き来する。LOG は左ペインがツリーではなくコミット一覧だが、新しい Focus variant は増やさず「左ペイン/右ペイン」という既存の意味を再利用する（GIT がツリーを変更ファイル一覧に絞り込むのと同じ考え方）
- 右ペインの中身はレーンで決まる（VIEW: ファイル / EDIT: 編集バッファ / GIT: diff / LOG: 選択コミットの diff）。`shell::draw` の振り分けがその唯一の場所。**LOG は左ペインもツリーから差し替わる**唯一のレーンなので、`draw_viewer_workspace` は他レーンより先に分岐して `tree_pane` 自体を呼ばない
- **未保存の編集バッファがあっても Workspace の切替は拒否しない**（`Lane::Edit` の状態はタブを跨いでも保持され、Viewer タブへ戻れば復元される）。Shift+Tab のレーン循環がバッファ dirty 中に拒否するのとは対照的で、その代わりタブ側に未保存マーク（`viewer ●`）を出す
- GitHub モードの有効化は起動オプション `--github` / 設定オーバーレイのトグル / config ファイル `github = true` の3経路が同じ `Config.github` に集約される。`--github` はその起動限りの上乗せで config には書かない（`App::github_enabled` と永続化用の `github_persisted` を分けて持つのはこのため）。`gh` の有無・認証・GitHub リモートかどうかの判定（`github::check_available`）は起動時（または初回有効化時）に1度だけ行い、描画のたびには叩かない
- `Mode::Help { scroll }` は**読み位置だけを持つ**。全レーン分のキー一覧は 169 行あり、端末の高さ（オーバーレイは 80%）に収まらない。以前は `Paragraph` に全行を渡すだけで、**溢れたぶん（Tree 以降のほぼ全セクション）が黙って切れていた**ため「Git のコマンドがヘルプに無い」ように見えていた。他のペインと同じく**自前でスライス**して描き（`Paragraph::scroll` は使わない）、実測（表示行数・総行数）を `App::help_view` へ書き戻して `on_help_key` がクランプとページ送り量に使う（`viewport.height` と同じ 描画→app のパターン）。枠のタイトルに `1-22/169` を出すのは、切れているのか終わりなのかを区別できるようにするため。専用の状態型を作るほどではないので `Mode::Commit` と同じくフィールドを直接持つ（＝ shell 側の画面）
- `Mode::Confirm { prompt, action }`（破壊的・書き込み系操作の確認）も Lane と直交する。これまでの Mode（Input/Finder/Help/Settings）は編集中は開けない制約があったが、Confirm だけは EDIT レーン中でも出す必要があるため、キールーティング上は Shift+Tab と同じ位置（`Lane::Edit` の文字入力ディスパッチより前）に置く。`action` はクロージャではなく enum（`ConfirmAction`）にする — クロージャだと App を借りたまま呼べず、確認後に App のメソッドを呼ぶ形にできないため。書き込み系の子 issue が増えるたびに variant を足していく想定（最初の実装が `ConfirmAction::Amend`）。確認中は y/Enter 以外の全キーで中止し、他のキーがレーンへ漏れないことをキールーティングの順序で保証する（型ではなく手続きで守っている点は他の Mode と同じ）
- `Mode::Commit { buffer, cursor, amend, error }`（コミットメッセージ入力、`c`/`C`）も Lane と直交する独立オーバーレイ。`Mode::Input` は 1 行入力専用で複数行のコミットメッセージを表現できないため分けた。`buffer` は改行込みの生テキスト、`cursor` はバイトではなく **char インデックス**（日本語等の複数バイト文字でカーソル位置がずれないため）。`c`/`C` はキールーティング上グローバルキー（q/s 等）と同じ位置に置き、GIT レーンにいることを要求しない（変更を見て回ってからそのままコミットしたい時に Shift+Tab を挟ませたくないため）。可否は `App::has_staged_changes` 等で都度判定する
- `Mode::Branch(BranchState)`（ブランチ一覧、`b`）も Lane と直交する独立オーバーレイ。`c`/`C` と同じ位置（グローバルキー相当）に `b` を置き、GIT レーンにいることを要求しない。`BranchState`（component/branch/mod.rs）は Finder と同じ「絞り込み候補 + 選択位置」のパターンだが、current マーク・local/remote 区別・upstream・相対日時・件名という Finder の `candidate: String` 1本では表現できない付随情報を持つため専用の型にし、マッチングだけ component/finder/mod.rs の `fuzzy_match`（`pub(crate)` に公開）を再利用して新しいマッチャを書かない。可否 (`App::branch_available`) は LOG と同じ基準（git repo でありさえすればよい）

### 閲覧と編集の関係（後付けにしない）
- `Viewport`（scroll/hscroll/wrap/実測サイズ）は閲覧・編集で**同じ実体を共有**する。モード遷移で位置が飛ばない根拠はここ。「wrap 中は hscroll = 0」のインバリアントは Viewport のメソッドと EditState::ensure_visible が守る（モード出口での手当てはしない）
- `Highlighter` は syntect のシンタックス定義とテーマの置き場で、ハイライト結果も行の状態も持たない。EditState は Viewer 全体ではなく **Viewport だけを借りる**（保存だけは cache 即時更新のため `Viewer::reload` を呼ぶ）。編集操作の経路（`handle_key`/`paste`）は Highlighter に触れず、借りるのは描画時（`component/editor/view.rs`）だけ。editor に新しい操作を足す時もこの依存範囲を広げない
- **ハイライトは「画面に映る 1 枚分」だけを描画時に組み立てる**（`component/viewer/render.rs` の `HighlightCache`）。閲覧・編集がそれぞれ 1 つ持ち、`TextPane` へは文書全体ではなく可視ウィンドウ（`widget/text_pane.rs` の `LineWindow`）を渡す。syntect は前の行の状態に依存する逐次処理なので、任意の行から再開できるよう `CHECKPOINT_STRIDE`(32) 行ごとにパーサ状態（`LineState` = ParseState + HighlightState）を保存し、可視範囲の直前の checkpoint から助走する。さらに「直近に組み立てたウィンドウ先頭の状態」（`resume`）を 1 つだけ別に持ち、STRIDE の境界に縛られない再開点として優先する — 下方向のスクロールと同じ画面での連続タイピングはこれで助走が 0〜1 行になる（上方向のスクロールだけは再開点を前に戻せないので STRIDE ぶんの助走を払う。STRIDE の値はここが数 ms に収まる範囲で選んである）。これで「開くコストがファイルの大きさに比例する」「編集の度に全再ハイライトする」の 2 つが同じ 1 つの仕組みで消える。checkpoint は要求された範囲まで歩く過程で埋まるので、末尾へジャンプした時だけ一度全体を歩き、以後は常に助走 ≤128 行で済む
- 無効化は 3 通りだけ: `reset`（ファイルを開く/読み直す）・`invalidate_all`（テーマ切替）・`invalidate_from(line)`（編集）。`invalidate_from` が「line より手前から始まる checkpoint は行番号がずれないので残せる」ことを使って、変更行より前を再ハイライトしないことを担保する。編集の起点はカーソル位置ではなく `EditBuffer::take_touched`（挿入・削除プリミティブが記録した最小の行）から取る — undo/redo は任意の位置に飛ぶため
- 描画は `widget/text_pane.rs` の `TextPane` に一本化（閲覧 = search あり cursor なし / 編集 = cursor あり search なし）。行加工順は `mark_changed_line → highlight_matches → (hscroll | セル単位 wrap) → cursor overlay` 固定
- wrap は閲覧・編集とも **セル単位（端末の表示幅単位）の自前分割**（`Paragraph::wrap` は単語境界 wrap で折返し位置が外から計算できないため全面的に不使用）。**折返し規則の唯一の定義は `text::WrapCursor`** で、描画（text_pane の `wrap_line` / gitlane の `wrap_split`）・視覚行数（`text::wrap_rows`）・カーソル追従（`text::wrap_position`）・クリック座標（`text::wrap_col_at`）の 4 者がこれを共有する。ズレると即カーソル位置バグになる
  - **char 数で詰めてはいけない**（実際に踏んだバグ）。ratatui の描画（`LineTruncator`）は grapheme の display width で桁を送り、幅を超えた時点でその行を打ち切るので、全角 1 文字を 1 桁と数えて詰めると視覚行が幅を超え、**はみ出した文字は次の視覚行にも現れないまま消える**（日本語のコメント行で「折返しの継ぎ目から数文字抜ける」形で表面化した）。全角が境界を跨げないぶん行末にセルが余ることもあるため、視覚行数も `display_len / width` の割り算では出せない（`wrap_rows` が本文そのものを受け取るのはこのため）
  - 1 文字のセル数は `text::char_cells`。unicode-width を直接足さず ratatui の `Span::width` を通すのは、描画側とまったく同じ計算であることを保証するため（新規依存も増やさない）

### キールーティングの優先順位（app/keys.rs on_key）
Ctrl+c → Mode::Confirm → Mode::Help → Mode::Settings → Mode::Finder → Mode::Input(Search/Goto/Filter) → Mode::Commit → Mode::Branch → **Ctrl+t/Alt+1..3(Workspace 切替)** → **Shift+Tab(レーン循環)** → **Workspace ≠ Viewer なら以降をスキップし on_issues_key/on_pr_key へ**（Issues は `on_issues_key`、PullRequests は `on_pr_key` がそれぞれ focus 別に一覧/詳細へ振り分ける。どちらも「フォーカスに依らない操作 (o/r/t/フィルタ開始) を先に拾い、残りは focus で振り分け」という同じ形） → Lane::Edit → Ctrl+p → q/?/a/i/s/c/C/b/f/p/P/Tab → **Z(stash pop、レーン不問)** → **X/z(discard・stash push、GIT レーン限定)** → focus 別ディスパッチ。f/p/P (#27 リモート操作) は c/C/b と同じ位置・同じ理由 (レーンを問わず開ける) でここに置く。新しいモード・キーを足す時はこの順序に組み込む。VIEW の範囲選択キー（v/y/Y/Esc）は `on_viewer_key` の中（focus 別ディスパッチの先）に置く — レーンを跨がない閲覧専用の操作なので、c/C/b のようにグローバルへ持ち上げない。Edit はグローバルキー（q/s/Tab/Ctrl+p）より前に置くことで印字キーを全て文字入力にしている（Ctrl+c と Shift+Tab だけが上に残る）。Shift+Tab をオーバーレイ判定より後ろに置いているのは、入力中にレーンが切り替わって文脈が壊れないようにするため。Ctrl+t/Alt+N も印字キーではないので同じ位置（オーバーレイ判定の後・Lane::Edit の前）に置ける。`workspace_available` が false の間はこれらのキーが素通りするだけなので、GitHub モード無効時の挙動は 1 バイトも変わらない。`pending_g`（gg 待ち）は Tree/Viewer で共用され、Tab・マウスでリセットされる。Z (stash pop) だけレーンを問わず呼べる理由は「破棄 (discard) と stash」節を参照。
GIT レーン右ペインの `Space`（hunk 単位ステージ）は `on_git_key` の中で、`A`/`t` と同じく `Lane::Git` の可変借用より前で拾う（git の実行と `rescan_now` に `&mut self` が要るため）。ツリー側（Focus::Tree）の `Space` はファイル単位のトグルで、粒度だけがフォーカスで変わる。
ツリーのキー処理（`on_tree_key`）は VIEW/GIT で共通で、**「開く」対象のパスを返すだけ**にしてある。viewer に開くか diff に開くかの振り分けは `App::open_selected` 1 箇所に閉じている（ツリー操作をレーンごとに複製しない）。LOG は左ペインがツリーではないため `on_tree_key` には乗せず、`Focus::Tree` の分岐で `Lane::Log` だけ `on_log_list_key` へ振り分ける（`Focus::Viewer` 側も同様に `on_log_diff_key` を割り込ませる）。

### 桁位置の整合インバリアント（複数ファイルに跨る前提）
- 各行 `Line` の **span[0] は行番号 gutter**。検索ハイライト・水平スクロールは span[1..] を char 単位で走査する
- `Content::Text(TextDoc)` の `plain` は normalize 済み（改行除去・タブ→スペース4）で、**char インデックスが「描画桁」= 検索マッチ・選択・カーソルが使う座標と 1:1 対応**する（全角文字は 1 char = 2 セルなので、端末のセル桁と一致するのは半角だけ。セル桁が要るのは折返し計算だけで、そちらは `text::WrapCursor` に閉じている）。検索マッチの (line, start_col, end_col) はこの前提で 描画側の bg 重ねに直結する。`TextDoc` はハイライト済みの `Line` を持たず（`raw` = syntect へ渡す生の行 と `plain` の 2 本だけ）、色は `HighlightCache` が可視範囲ぶんだけ後から付ける。だからテーマを切り替えても `HashMap<PathBuf, Rc<Content>>` の cache は捨てなくてよい
- タブ幅・gutter 幅・表示桁⇔char 座標の換算は **`text.rs` が唯一の定義**。閲覧（component/viewer/content.rs の normalize）と編集（カーソル・クリック座標）が別々に持つとここが最初に壊れる
- 大文字小文字の畳み込みは ASCII 限定（`to_ascii_lowercase`）。Unicode の完全 case folding は char 数が変わり桁対応が壊れるため意図的に使っていない（component/viewer/search.rs と component/finder/mod.rs の両方）
- text_pane の行加工順は `mark_changed_line → highlight_matches → highlight_selection → hscroll_line` 固定。hscroll を先にすると検索マッチ・選択範囲の絶対桁がズレる
- gutter の変更行マーク `▎` は「gutter 末尾の空白 1 文字を置き換える」方式で char 数を維持している

### 描画の依存範囲（View は自分の状態しか受け取らない）
各ペイン・オーバーレイの `draw_*` は **そのコンポーネントの状態 + 描画に要るスカラ（`focused` / `background` 等）だけ**を引数に取り、`&App` は受け取らない。理由は 2 つある。
- **借用**: `GitState` も `EditState` も `App` の中にあるので、`&App` と `&mut app.lane` は同時に取れない。呼び出し側（`shell::draw`）が先に必要な値を取り出してから子へ渡す形にしないとそもそもコンパイルが通らない（`component/gitlane/view.rs` の先頭コメントがこの経緯）
- **範囲を型で縛る**: View が触れる状態が引数の型で決まるので、「このペインを直すのにどこを見ればいいか」がシグネチャだけで分かる。App 全体を渡すと、あとから無関係なフィールドを読み始めても誰も気づけない

例外は **シェル側の画面**（`shell/` 配下: status_bar / settings / confirm / commit / tab_bar）で、これらは本質的に App 全体の状態・設定を横断して見せるものなので `&App` のままにしてある。「専用の状態型を持つか」が境界の目安で、`Mode::Finder(Finder)` / `Mode::Branch(BranchState)` のように状態型がある側はコンポーネント扱い、`Mode::Commit { .. }` のように `Mode` の中に直接フィールドを持つ側はシェル扱いになる。この境界はそのまま**フォルダの置き場所**（`component/` か `shell/` か）と一致する。

### 描画は自前スライス
`Paragraph::scroll` は u16 上限で使わない。`lines[scroll..scroll+height]` を毎フレームスライスして描画する（text_pane）。ui は `viewport.height` / `viewport.width` / `tree_area` / `viewer_area` / `splitter_area` を毎フレーム App/Viewport に書き戻し、キー・マウス処理側がそれを読む（描画→app の逆流はこのパターンに統一）。

### ペイン幅のドラッグリサイズ
左右の比率は `App::split_ratio`（config に永続化）。桁数でなく割合で持つのは端末リサイズで配分を保つため。割合→実桁の換算は `App::tree_width` 1 箇所だけで、ドラッグ時の clamp（`clamp_tree_width`: 最小幅を満たせない狭い端末では半分ずつ）も同じ関数を通す。ドラッグは `on_split_mouse` がレーン・オーバーレイ判定より前に処理して消費する（幅変更はレーンと直交する操作。編集中でも効かせる）。掴んだ桁のオフセットを `dragging_split` に持つので Down の瞬間に境界が飛ばない。config への書き込みはボタンを離した時だけ（ドラッグ中に毎フレーム書かない）。

### ツリー走査と FS 監視（起動をディレクトリの大きさから切り離す）
巨大なディレクトリで開くのに数秒かかっていたため、**起動時に触るのは root 直下 1 階層だけ**にしてある。「起動時にツリー全体を歩く」処理を足さないこと（`App::new` の所要時間がツリーの大きさに比例しない、が守るべき性質）。
- 走査は `scan::read_dir` の **1 階層ずつ**。`NodeKind::Dir` の `loaded` が未走査を表し、`scan::load` が展開の直前（`toggle_or_open` の開く側、`expand_all`）で子を読む。畳んだ子は捨てないので再展開はキャッシュヒットになる
- 1 階層でも `WalkBuilder` を通すのは、既定の `parents(true)` が**祖先の .gitignore を遡って読む**ため。サブディレクトリ起点でも root 側の `*.log` / `/anchored` / `build/` がそのまま効く（この前提が崩れるなら一括走査に戻すしかない）。`require_git(false)` で非 git ディレクトリでも .gitignore を尊重
- `rescan` は `scan::refresh` で**読み込み済みの階層だけ**を読み直し、展開状態と子を **name で**引き継ぐ（種別が変わったら引き継がない）。選択は **path で**保存・復元する（index_path は再走査で無効になる）。再走査コストも「今開いている範囲」に比例する
- `toggle_hidden` は show_hidden を反転して `rescan` するだけ（読み直しの経路を 2 つ持たない）。`toggle_ignored`（`i` / 設定画面の gitignored）も同じ形で、切替後の後始末（Finder 候補・FS 監視をツリーと同じ条件に揃え直す）は `App::after_scan_options_changed` 1 箇所に集約する
- **無視設定は `scan::ScanOptions`（show_hidden + show_ignored）が唯一の定義**で、`ScanOptions::walker` が `WalkBuilder` の組み立てを持つ。ツリー・Finder の候補（component/finder/index.rs）・FS 監視（watch.rs）の 3 者がこれを共有するのは、条件がずれると「ツリーには出るのに Finder に出ない」「表示しているのに自動リロードだけ効かない」が起きるため。bool を個別に配って回らない
- **無視ファイルの表示（`i`）は走査を切り替えるだけでなく「どれが無視対象か」も要る**（暗色で区別するため）。ignore クレートの走査結果にはその情報が無いので、`scan::read_dir` は同じ 1 階層を「無視を効かせた設定」でもう一度歩き、そちらに出てこなかったものを無視対象と見なす。パターンの解釈（否定・アンカー・祖先の .gitignore）を自前で持たず、表示・非表示と完全に同じ判定を使うのが目的。追加コストは **show_ignored が on の間だけ**の 1 階層ぶんの readdir 1 回で、無視されたディレクトリの配下は git 的にも全て無視対象なので `parent_ignored` を伝播させて再走査自体を省く
- 監視の開始（notify の再帰 watch 登録）も**ツリーの大きさに比例する**ため別スレッドに出し、`FsWatcher::drain` が毎 tick 受け取りに行く。登録完了までのイベントは取りこぼすが、それは監視開始前と同じ状態でしかない

### GitHub モードのタブバー（app/mouse.rs on_tab_mouse・shell/tab_bar.rs）
タブごとの列範囲は `App::tab_areas`（`shell/tab_bar.rs` が毎フレーム書き戻す、`tree_area`/`splitter_area` と同じ 描画→app のパターン）。クリック判定 `on_tab_mouse` はペイン境界のドラッグと同じ理由でレーン・オーバーレイ判定より前に処理して消費する（タブ移動はレーンと直交する操作）。`workspace_available` が false の間は `tab_areas` が全て空 Rect のままなので、判定コードを分岐させなくても自然に無効化される。

### issues タブ（#33、github.rs + component/issues/）
- 取得は `gh issue list`/`gh issue view` を CLI 呼び出しで済ませ、`--json` の生 JSON を自前パースする代わりに `--template` で `\0` 区切りのプレーンテキストへ整形させてから porcelain -z と同じ流儀でパースする（serde を足さないため）。`--template` 単独では gh が使うフィールドを決められないため `--json number,title,author,updatedAt,labels,state,body` を必ず併せて渡す
- **一覧は `--state all` で常に 1 回だけ取得する**。`t`（open/closed/all の循環）は再取得せず `IssuesState::state_filter` によるローカルフィルタに閉じる — 「タブを往復しても gh を叩かない」という要求と同じ理由で、state 切替のたびに gh を叩くのは避けたい。副作用として `--limit 100` の枠を open/closed 合算で消費する（極端に issue が多い repo では新しい open issue が一覧から漏れうるが、`r` で明示的に取り直せる）
- `RemoteItem`（github.rs）は issue 固有の項目を持たない一覧行の型。`gh issue list`/`gh pr list`（#34）はどちらも同じ `--json` フィールド名（number/title/author/updatedAt/labels/state/body）を返すため、#34 が型を分けずにそのまま再利用する。一覧側の絞り込み・スコアリング・詳細の非同期キャッシュは `component/remotelist/mod.rs`（`filter_rows`/`DetailSlot`、次節参照）へ切り出し、issues/PR 両タブが実装を共有する。issue 固有なのは詳細の組み立てと state 絞り込みのカーディナリティ（open/closed/all）だけに閉じている
- フィルタ（`/`）は component/branch/mod.rs::BranchState と同じパターンで、新しいマッチャを書かず `component/finder/mod.rs::fuzzy_match` を再利用する。Search（`Mode::Input`）と同じ枠組みに乗せるため `InputKind::Filter` を追加したが、意味は Search と異なる：一覧の絞り込みは編集を始める前から existing な「常設状態」なので、Esc は Search の「全消去」ではなく「編集を始める前のクエリへ復元」にした（`IssuesState::begin_filter_edit`/`cancel_filter_edit` が snapshot を持つ）
- **体感速度改善（詳細を開く往復を 2 回 → 1 回に削減）**: 当初は Enter で開くたびに `gh issue view <n>`（本文）と `gh issue view <n> --comments`（コメント）の 2 回叩いていたが、本文は一覧取得の時点で `RemoteItem::body` として既に受け取っているので、開いた瞬間に rows から即座に本文を組み立てて描く（`IssuesState::rebuild_display`、ネットワーク往復ゼロ）。コメントだけを非同期の 1 往復 (`github::issue_comments`) で取りに行き、届くまでは本文の下に「コメント読み込み中…」を添える (`issues::build_detail_display`)。届いたら viewport はリセットしない（読んでいる途中でスクロール位置が飛ぶのを避けるため、request_open 時点で既に先頭にリセット済み）
- `--json` に `body` を渡すと失敗する古い gh 向けに、`github::list_issues`/`list_prs` は一覧取得が失敗したら body 抜きの従来テンプレートで 1 回だけ再試行する（この時 `RemoteItem::body` は空文字のまま = 「(no description)」表示になる）。一覧全体が出なくなる方が体感速度より悪いため、フォールバックを安全側として必ず入れる
- ブラウザで開く (`o`) は `job.rs` (#27) に乗せる。コメントは issue 番号ごとに `DetailSlot`（component/remotelist/mod.rs）で `HashMap` キャッシュし、選択を変えても再取得しない。**未キャッシュ・未取得中のときだけ** job を起動する判定 (`IssuesState::request_open` → `DetailSlot::request`) に一本化し、Enter を連打しても二重起動しない。取得失敗はキャッシュに残さない (`errors` は成功でクリアするだけ) ので、再度 Enter で再試行できる
- 詳細の `Viewport` は VIEW/EDIT・GIT・LOG のいずれとも独立 (`IssuesState.viewport`)。他レーンと違い折返しトグル (`w`) を割り当てていない — issue 本文はコード行と違い折返しが基本的に必要な prose なので、常に `wrap = true` で固定し config にも保存しない
- Focus (Tree/Viewer) は Lane 用に定義された既存の enum をそのまま流用する（新しい variant を増やさない。GIT/LOG が「左ペイン/右ペイン」の意味を再利用するのと同じ考え方）。Workspace::Issues に入った瞬間は GIT/LOG と同じく Focus::Tree（一覧側）に寄せる
- `j`/`k` で詳細ペインを自動追従させない（Enter/l/クリックでのみ開く）。GIT のツリー・LOG の一覧と同じ理由（キーリピートで gh を連打しないため）
- 一覧の描画は「タイトルを最優先で残し、狭い端末では author → 更新日時 → labels の順に列を落とす」（`component/issues/view.rs` の閾値定数）。char 単位のファジーマッチ位置ハイライトは branch_panel.rs::highlight_name と同じ組み立て方
- gh 未インストール/未認証/GitHub リモートでない場合は `github::check_available`（既存、#32）が起動時に一度だけ弾くため、issues タブ自体がタブバーに現れない。component/issues/mod.rs 側の取得コードは「gh は動く前提」で書いてよく、ここでの失敗はネットワーク断・API レート制限・issue 権限等の実行時エラーに限られる（`list_error`/`detail_errors` で表示するだけで panic しない）

### issues/PR 一覧の共有基盤（component/remotelist/）
- issues（#33）と pull requests（#34）は「gh の一覧取得 → フィルタ（query + state）→ 選択 → 詳細を番号ごとに非同期キャッシュ」という形が完全に同じなので、フィルタ・スコアリング（`filter_rows`）と詳細の非同期キャッシュ（`DetailSlot<T>`）だけを共有モジュールへ切り出した。一覧の型そのもの（`Vec<RemoteItem>`/`Vec<PrRow>`・`ListMatch`・`ListState`・`selected` 等の各フィールド）は `IssuesState`/`PrsState` にそれぞれ持たせたまま重複させている — Rust には構造体フィールドの継承が無く、フィールドをジェネリック構造体へ完全に統合しようとすると `pub` フィールドの直接アクセス（下記）が要求する「同じ変数の複数フィールドを 1 回の関数呼び出しで同時に借りる」という制約と衝突し、かえって複雑になるため
- **`filter_rows<R: ListRow>`**: クエリでスコアリング → state 述語 (`accepts: impl Fn(&str) -> bool`) で絞り込み、という 2 段階のアルゴリズムそのものを共有する。state 絞り込みのカーディナリティ（issues は open/closed/all、PR は open/closed/merged/all）は呼び出し側の `StateFilter`/`PrStateFilter` に閉じ、`filter_rows` へは述語として渡すだけにすることで、この違いを共有関数に持ち込まない。`ListRow`（title/state を返すだけの trait）は `RemoteItem` に実装済みで、PR の一覧行 (`github::PrRow`: `RemoteItem` + headRefName/isDraft) にも `component/prs/mod.rs` 側で実装し、`RemoteItem` 自体を PR 専用フィールドで汚さない
- **`DetailSlot<T>`**: 番号ごとに「取得中/キャッシュ済み/エラー」を持つ非同期キャッシュ。issues のコメント（1 種類）と PR のコメント/diff/CI（3 種類、`PrsState` が 3 つ持つ）がどちらも同じ形なので、組み立て済みの表示データ型 `T` だけ差し替えて共有する。`request(number) -> bool` が「未キャッシュ・未取得中の時だけ true」を返す一本化された判定で、Enter/d/S の連打でも二重に job を起動しない。**体感速度改善以降は「本文込みの詳細」ではなく「コメントだけ」のキャッシュ**になっている — 本文は一覧取得済みの `RemoteItem::body` から毎回即座に組み立てるのでキャッシュを要らない（issues タブの節参照）
- **`rows`/`matches`/`list_state` は `pub` フィールドで直接公開する**（getter メソッドにしない）。`component/remotelist/view.rs::draw_remote_list` は 1 回の呼び出しで「`rows`/`matches` を読みながら `list_state` を書く」必要があるが、`&self` を取るメソッド越しだとコンパイラには「self 全体を借りている」ようにしか見えず、同じ呼び出しの中で別フィールドへの `&mut` と共存できない（借用エラー）。直接のフィールドパス（`&issues.matches`、`&mut issues.list_state`）にすることで、コンパイラがフィールド単位の互いに素な借用として認識できるようにしている。同じ理由で `list_error()`（`Option<&str>` を返す）は呼び出し直前に `.map(str::to_string)` で複製してから渡す（エラー時にしか複製しないので実害はない）
- `component/remotelist/view.rs`（`draw_remote_list`/`draw_text_detail`）が描画側の共有部分。一覧行の表示テキスト組み立て（`issue_line`/`pr_line`）は型ごとに差し替えるクロージャとして渡す形にし、issues/PR で「一覧の描画・絞り込み・キャッシュを 2 回書かない」という受け入れ条件を満たす

### pull requests タブ（#34、component/prs/）
- 取得コマンドと詳細描画だけを issues タブと差し替える形にしてある。一覧まわり（フィルタ・キャッシュ・描画）は前節の component/remotelist/mod.rs 経由で完全に共有し、PR 固有なのは `github::PrRow`（headRefName/isDraft）・`PrStateFilter`（4 値）・右ペイン 3 種の切替・diff の hunk/wrap/hscroll だけ
- 右ペインは 3 種類の表示を切り替える: （既定）説明、`d` = 差分 (`gh pr diff`)、`S` = CI ステータス (`gh pr checks`)。切替は `PrsState::view: DetailView` が持ち、`set_open(number, view)` が「対象 PR・表示のどちらかが変わったら、その表示が使う Viewport の読み位置をリセットする」を一箇所に閉じる。`d`/`S` は開いている（無ければ選択中の）PR を対象にするので、Enter を経由しなくても一覧から直接差分/CI を読み始められる
- **説明表示も issues の詳細と同じ体感速度改善が入っている**: 本文は一覧取得済みの `RemoteItem::body`（`PrRow::item.body`）から即座に組み立て (`PrsState::rebuild_description_display` → `issues::build_detail_display` を共有)、コメントだけ `gh pr view --comments` (`github::pr_comments`) の非同期 1 往復で取りに行く。`comments: DetailSlot<Vec<Line>>` フィールドの役割はコメントキャッシュに変わっており (`description` から改名)、`loading_current`/`error_current` は Description の間は常に false/None を返す (全体ブロックする理由が無くなったため。コメントの取得中/失敗は `description_display` の中に埋め込まれている)。diff・CI は一覧に含まれないデータなのでこの改善の対象外だったが、下記の**先読み**で `d`/`S` を押した瞬間の 1 往復待ちも別途無くしてある
- **diff/CI の先読み（`PrsState::advance_prefetch`、`PrefetchStage`）**: Enter/l/クリックで PR を開いた瞬間 (`note_opened`) から ~400ms 後 (`PREFETCH_DELAY`) に、diff → CI の順で静かにバックグラウンド取得を始める。`d`/`S` を押した時点で既にキャッシュに入っている状態を作るのが目的で、押した瞬間の 1 往復待ちが本題（先読み自体は #34 の「取得が要る」制約を変えていない、取得のタイミングを早めているだけ）。要件は 3 つ: (1) 起点は Enter/l/クリックの明示操作だけ — `note_opened` を呼ぶのは `App::open_selected_pr` のみで、j/k の選択移動では呼ばない（キーリピートで gh を連打しない、GIT/LOG のツリー・一覧 j/k と同じ理由）。(2) 開いてすぐ撃たず ~400ms 後に発火 — Enter 連打で一覧を流し読みする使い方の無駄弾を防ぐため、`note_opened` は呼ぶたびに前のタイマーを新しい対象で上書きする（`PrefetchStage::Pending(number, Instant)`）。古い対象への移行後の続き（diff 完了後の CI 着手）も `open_number` が対象からずれていたら中断する。(3) 同時に走らせるジョブは高々 1 本 — `PrefetchStage` は `Pending → DiffInFlight → ChecksInFlight` と一方向にしか進まず、diff のジョブが終わるまで CI のジョブは起動しない。`DetailSlot::request` の既存の重複防止 (`request` が false を返せば次の段階へ即座に進む) をそのまま使うので、既に `d`/`S` で明示的に取得済み・取得中の対象は先読み側からは何も撃たない
- **先読み中であることを UI で主張しない**: `note_opened` 直後は `view` が常に `Description` のままなので、`loading_current`/`error_current`（`self.view != Diff/Checks` の間は素通りする既存の view ゲート）が自然に「先読み中は何も見せない」を満たす。`d`/`S` を押した時点でまだ取得中なら、その時初めて `self.view` が Diff/Checks に変わり「読み込み中…」が出る——新しいフラグは要らない
- **先読みの失敗も notice を出さない**: `DetailSlot` は失敗をキャッシュに残さない (`errors` は成功でクリアするだけ) ので、`d`/`S` を押した時点の `request` が「未キャッシュ」として自動的に再取得を試みる。先読み固有のエラー処理は書いていない（issues タブと同じ「失敗はキャッシュしない」設計にただ乗りしている）
- **巨大 diff の打ち切り notice は先読み経由では出さない**: `PrsState::poll` の通知は `truncation_notice_if_needed` が `self.view == DetailView::Diff` の間だけ許可する共有判定に一本化してある。先読み完了時点では `view` が `Description` のままなので通知は素通りし、`notified_truncation: HashSet<u64>` に積まれない。`d` を押して初めて表示を切り替える瞬間、`App::switch_pr_view` が `dispatch_pr_fetch` の直後に `truncation_notice_for_current` を呼んで同じ判定をかける——先読みで既にキャッシュ済みだと `dispatch_pr_fetch` はジョブを起動せず (`poll` 側の通知も発火しない) ため、表示に切り替えた瞬間にここで打ち切りを知らせないと機会を逃す。番号ごとに 1 度だけ通知するのは `notified_truncation` の集合で担保する
- `App::on_tick` は毎 tick `dispatch_pr_prefetch`（`advance_prefetch` が返した対象を job::spawn するだけの薄いラッパー）を呼ぶが、タイマー未到達で `None` が返る間は `changed` を立てない——アイドル時に毎 tick 再描画すると CPU を焼く（#53 で潰した問題）ため、実際にジョブが完了して `poll` 側の `outcome.changed` が立った時だけ再描画すれば足りる
- 説明/CI ステータスは issues の詳細と同じくプロースなので `text_viewport`（常に wrap 固定）を共有する。**diff だけ別の `diff_viewport` を持つ**（GIT/LOG と同じ「別ドキュメントなので位置を共有する意味がない」という理由）。`w`（折返し）・`h`/`l`（hscroll）・`]`/`[`（hunk ジャンプ）は diff 表示中だけ効き、`PrsState` 側の対応メソッド（`toggle_diff_wrap`/`hscroll_by`/`next_hunk` 等）が `self.view != DetailView::Diff` の間は no-op にすることで、キールーティング側 (`app/keys.rs::on_pr_detail_key`) は表示の種類を意識せず同じキーを渡すだけで済む
- **diff は `gitlane::render_commit`（LOG レーンの複数ファイル diff・GIT レーンの「全ファイルまとめ diff」と共有しているレンダラ）にそのまま通す**。`gh pr diff` の出力は `git diff`/`git show` と同じ unified diff 形式なので、行の組み立て（span[0] = gutter・新側行番号・削除行は空欄・word-level ハイライト）を複製しない。**sticky header（`widget/diff_boundary.rs`）も同じ部品をそのまま使う**——`gitlane::sticky_label` で境界一覧を引き、`component/prs/view.rs::draw_pr_diff` が LOG/GIT の diff ペインと同じ組み立て順（`widen_boundary_bands` → 先頭に `sticky_line` を挿す）で描く。`src/component/gitlane/view.rs`/`src/component/log/view.rs` 自体は触らず、同じ発想の描画コードを `component/prs/view.rs` に新規で書く形にした（他レーンの描画ファイルを直接 import できる構造ではないため）
- **巨大 diff の打ち切りは `git::truncate_diff` を再利用する**（`A`「全ファイルまとめ diff」と同じ 20000 行 / 2MB の上限）。`git.rs` 側の関数を `pub(crate)` に上げただけで、判定ロジック自体は複製していない。取得ジョブ（`component::prs::fetch_diff`、バックグラウンドスレッド側）が打ち切り + `render_commit` まで済ませてから `PrsState` へ渡すため、メインスレッドは結果を表示するだけで良い。打ち切りが起きたら `PrsState::poll` が notice 用のメッセージを返し、ペインタイトルにも `(打ち切り)` を添える
- `gh pr checks` は失敗中のチェックがあると非ゼロ終了するため、`github::pr_checks` は終了コードでは判定せず「stdout が空でなければ使う」を優先する（失敗を隠さずそのまま見せるのが目的）
- 一覧行は `#番号 [draft] タイトル @author ブランチ名 更新日時` の順で、狭い端末では右側の列（author → ブランチ名 → 更新日時）から落とす。issues の `issue_line` と同じ「タイトルを最優先で残す」閾値方式（`component/prs/view.rs` の定数）。state の色分けは open/merged/closed の 3 値（issues は open/closed の 2 値なので `issue_line`/`highlight_title` をそのまま使えず、`pr_line`/`state_style` として別に持つ）

### ツリー走査と FS 監視
- 走査は起動時に WalkBuilder 1 回で一括（サブディレクトリ起点の遅延走査だと親の .gitignore が効かない）。`require_git(false)` で非 git ディレクトリでも .gitignore を尊重
- `rescan` は展開状態と選択を **path で**保存・復元する（index_path は再走査で無効になる）
- **削除された（worktree または index で `D`）が未コミットのファイルは合成ノードとして Tree に足す**（`Tree::sync_deleted`）。WalkBuilder は実ファイルしか見ないため、`rm` 等で既に消えたパスは通常の走査に一切出てこず、このままでは GIT レーンで選択も stage/unstage もできない。Tree は本来 git を知らない設計だが、削除ファイルの可視化だけはこの橋渡しが無いと表現できないため例外的に許容する。`App::rescan` / `App::new` / `toggle_hidden` が nodes を作り直す（＝合成ノードも失う）都度、最新の git status から呼び直す設計で、専用の同期タイマーは作らない。**削除集合は `Tree` が持ち続け、実際の挿し込みは `rebuild_visible` が毎回行う** — 合成ノードを失うのは rescan だけでなく**遅延ロード（`scan::load` が実走査の結果で children を丸ごと置き換える）でも起きる**ため。「起動時に展開されていないディレクトリ配下の削除ファイルが、そのディレクトリを開いた瞬間に消える」という形で表面化していた（1 回挿して終わりにはできない）
- watch.rs のイベントフィルタは「`.` 始まり成分の除外 + root .gitignore の `matched_path_or_any_parents`」（`matched` だと `target/` が配下パスに効かない）。ツリー再走査は 500ms デバウンスで、git status の再取得もこれに相乗りする（別タイマーを作らない）
- **`FsWatcher::drain` はイベントを「構造変化 (作成・削除・リネーム)」と「内容だけの変更 (Modify(Data))」に分類して返す**（`watch::Change { path, structural }`）。ファイルの中身が変わってもツリーの行構成（どのパスが存在するか）は変わらないため、`App::on_tick` は structural なイベントが 1 件も無ければ `tree.rescan`（WalkBuilder の全走査）を丸ごとスキップし、`App::rescan_status_only`（git status の再取得 + GIT レーンの絞り込み・diff 更新だけ）で済ませる。大きい repo では「AI が高速に書き換え続ける」ような内容変更の連打が全走査の主なコストだったため、ここを削るのが効く。`Modify(Metadata)` は従来通り完全無視、種別が判別できない Modify は安全側 (structural) に倒す — 誤って全走査を省略し表示が古いまま固定される事故より、たまに余計な全走査をする方が無害なため
- **GIT レーンの絞り込み・diff は status ベースで足りる**ので、内容変更だけの tick でも `tree.set_filter`/`GitState::refresh` は毎回呼ぶ（`App::after_status_refresh`、rescan/rescan_status_only 共通）。「新しく変更されたファイルが絞り込みに現れる」という要求は `GitStatus.files`（`git status` の出力）だけで満たせ、ツリーの再走査は要らない。以前は「GIT レーンにいる間は変更が 1 件でもあれば無条件に rescan_pending を立てる」という特別扱いがあったが、この分類導入後は不要になった（全ての内容変更イベントが既に `after_status_refresh` を通るため）ので削除した
- 削除・作成・リネームは常に structural 扱いで `rescan()`（全走査）側に回るため、`tree.sync_deleted`（削除ファイルの合成ノード追加）は `rescan_status_only` では呼ばない。内容変更だけの tick では新しく削除されたパスが発生しない前提
- **「今開いているファイルの reload」と「再走査/status 再取得の保留フラグ」は排他にしない**（`App::on_tick` のイベント分類）。以前は `if 開いている path { viewer.reload } else if structural { .. } else { .. }` と繋がっていたため、**閲覧・編集中のファイルを書き換えても git status が再取得されず**、差分の有無に依存する表示（GIT レーンの可否 = `git_available`・ツリーの status・diff）が `r` を押すまで古いままだった。開いているかどうかは「viewer の cache を捨てるか」を決めるだけで、git 側の追従が要るかどうかとは独立している
- **fv 上での保存 (`Ctrl+s`) も FS 監視のイベント待ちにしない**。`EditState::save` が立てる take フラグ (`EditState::take_saved`) を `App::on_edit_key` が回収して `status_pending` を立てる（監視を張れない環境でも効かせるため）。ファイルの増減は起きないので全走査は要らず、再取得自体は `on_tick` の 500ms デバウンスに任せる（連続保存で git を連打しない）。EditState は App を借りられない（「閲覧と編集の関係」節の依存範囲）ので、`EditBuffer::take_touched` と同じ take フラグで橋渡しする

### Finder の候補（component/finder/index.rs）
ツリーが遅延走査になったので、`Ctrl+p` の候補をツリーから集めると未展開の階層が丸ごと欠ける。`FileIndex` が root 全体を**別スレッドで 1 回歩いて**候補を持つ（無視設定はツリーと同じ `ScanOptions::walker` を通すので、隠し項目・無視ファイルの表示切替がそのまま候補にも効く）。
- 走査を起こすのは Finder を開いた時だけ（起動時に走らせると、使わないのに巨大ディレクトリを歩くことになる）
- 走査完了前に開いた場合は**ツリーの読み込み済み分**で即座に開き、完了時に `on_tick` が `Finder::set_candidates` で差し替える（クエリは保つ）。タイトルの `scanning...` がその状態
- FS 変更・隠しファイル切替では `invalidate` するだけ。ここで走査し直すと保存のたびに全走査になる（古い一覧は次に Finder を開くまで使い続ける）

### ツリーペインの描画（component/tree/view.rs）
- **`ListItem` の組み立ては画面に映る行数に比例させる**（以前は `tree.visible` 全体に比例していた。展開済みの巨大なツリーで `j` を押しっぱなしにすると 1 回の再描画あたり `visible` 全件ぶんの `format!`/`Vec` 確保が走り、キー入力への追従が目に見えて遅れていた）。`ListState` の scroll/offset 管理は ratatui の `List` に任せず自前に持ち替えた（下記 A 案）。B 案（組み立て済み `Vec<ListItem>` をキャッシュし内容が変わった時だけ作り直す）も検討したが、A 案の方が「常に O(画面行数)」を型で保証できて strictly 強く、`List::new` が `Vec<ListItem>` を所有として消費する ratatui の API 上、キャッシュを毎フレーム使い回すにも結局クローンが要って B 案の優位性が薄れるため見送った
- ツリーの行は高さが常に 1 (`row.name` に改行は入らない) という前提があるので、ratatui `List` が内部でやる「選択行を含む最小限のウィンドウを保つ」スクロール計算 (`get_items_bounds`、非公開 API) は、offset を起点に selected が入るまで前後にスライドさせるだけの O(1) の式に厳密に置き換えられる（`component/tree/view.rs::visible_window`）。この式は ratatui 側のテストケース (`selected_item_ensures_selected_item_is_visible_when_offset_is_*`) の期待値と突き合わせて導出した。可変高さ行 (`repeat_highlight_symbol`・複数行アイテム等) は使っていないので、この前提が崩れる変更 (行を複数行にする等) をする時はこの等価性も一緒に見直すこと
- `[first, last)` の絶対 offset は `app.tree.list_state`（`offset_mut()`）に書き戻す。`app/mouse.rs::click_tree_row` がクリック行の絶対 index 換算にこの offset を読むため（`tree_area`/`viewport.height` などと同じ 描画→app の書き戻しパターン）。`List` 自体には `[first, last)` にスライスした部分列と、それに合わせて相対化した選択位置を持つ使い捨ての `ListState` を渡す — `List::new` が受け取った `Vec<ListItem>` をそのままインデックス 0 起点として扱うため、絶対値の `list_state` をそのまま渡すと選択位置も offset も二重にずれる
- 選択のハイライトは `List::highlight_style` が描画時に当てるだけで `ListItem` 自体には焼き込まれないため、`j`/`k` で選択が動くだけなら（＝ウィンドウの範囲が変わらなければ）以前と同じ行の `ListItem` を作り直しても意味が無い。今回のウィンドウ縮小と合わせて、実質的に「画面外の行は最初から作らない」形になっている

### GIT レーン（component/gitlane/）
- 左ペインは `Tree::set_filter` による**表示フィルタ**（変更ファイル + その祖先ディレクトリ）。集合は `GitStatus.files` と `changed_dirs` の和で、ツリーの再走査はしない。ただし変更ファイルが未展開の階層にいることはあるので、`expand_all` は集合に含まれるディレクトリを**開く直前に読み込む**（`changed_dirs` が祖先を全部含むので root から辿れる）
- 絞り込み中も `expanded` フラグを尊重するので h/l/H の開閉がそのまま効く。代わりに `set_filter` が**絞り込み開始時に元の展開状態を退避 → 対象を全展開**し、解除時に `scan::set_expanded` で厳密に戻す（GIT 内での開閉は VIEW に持ち越さない）。絞り込み中の再走査では「新しく対象になったディレクトリ」だけを開き、ユーザーが畳んだものは保存のたびに開き直さない
- 右ペインは `git diff <base> -- <file>`（`git::DiffBase`: Head/Staged/Unstaged）を `TextPane` の行形式（span[0] = gutter、gutter は新側行番号・削除行は空欄）に組み替えたもの。untracked の `--no-index` フォールバックは Head/Unstaged のときだけ（Staged は「index にまだ無い」が正しい状態なので出さない）
- **diff 基準（`GitState::base`）は GIT レーン内だけの一時状態**。`t` で HEAD → staged → unstaged と循環し、ペインタイトルに常に出す。`w`（折返し）と同じく config には保存しない。**`changed_lines`（VIEW の gutter マーク）・`baseline_lines`（EDIT のライブ diff）は意図的に HEAD 固定のまま**で `DiffBase` に連動させない（GIT レーンの操作で閲覧・編集の変更行マークが勝手に変わるのを避けるため）
- **diff は VIEW/EDIT が共有する Viewport とは別の Viewport を持つ**（GitState 内）。別ドキュメントなのでスクロール位置を共有する意味がなく、VIEW に戻った時の読み位置も壊さない。`w`（折返し）も GIT 内だけの独立トグルで config には保存しない
- ツリーの status 表示は `FileStatus { index, worktree }` で porcelain の XY を index 側 / worktree 側に分けたまま持つ（`M ` / ` M` / `MM` / `??` の 2 文字表示）。1 種類に潰すと「ステージ済みかどうか」が表現できず staged/unstaged diff の切替と食い違うため。色は worktree 側（未ステージ）を優先して判定する
- ツリーの j/k で diff は追従しない（Enter/l/クリックで開く）。キーリピートで git プロセスを連打しないため
- 絞り込みと diff の再取得は FS 監視の 500ms デバウンス（`App::rescan`）に相乗りさせる。専用タイマーを作らない
- **`Space` は選択ファイル/ディレクトリの stage/unstage トグル**（`App::toggle_stage_selected`、Focus::Tree 限定）。判定は「worktree 側に未ステージ変更が残っているか」（無ければ unstage）で、ディレクトリは配下の `git.files` を集約して同じ判定に使う。コマンドは `git::stage_path`（modified/untracked は `git add --`、削除を含むときは `git add -A --`）/ `git::unstage_path`（`git restore --staged --`、HEAD の無い初期 repo は `git rm --cached --` へフォールバック）。非破壊的（いつでも打ち消せる）操作なので `Mode::Confirm` は経由させない — 経由させると「連打でサッと trial-and-error する」という stage/unstage の使い方自体が壊れるため
- Space はキーリピートで git プロセスが暴走しないよう `STAGE_DEBOUNCE`（150ms）で間引く。`j`/`k` のような移動キーは「頻度が高いので git を叩く側を分離する」（診断済みの既存方針）で対処できるが、Space 自体が実行キーなのでこの手が使えず、実行キー本体に debounce を持たせている点が他のキーと違う
- 実行後は既存の `App::rescan`（r キーと同じ入口）にそのまま乗せる。`Tree::rescan`／`Tree::set_filter` の path ベース選択復元がそのまま「選択位置を飛ばさない」「絞り込みから外れたら近い残存行に寄せる」を満たすので、stage/unstage 専用の位置合わせロジックは書いていない
- **右ペイン（Focus::Viewer）の `Space` は hunk 単位の stage/unstage**（`App::stage_current_hunk`）。ツリー側の Space と同じキーに粒度違いを割り当てているのは、「ファイルを選んで Space」「diff を読みながら Space」がフォーカスで自然に切り替わるため。#20 で「実装が重いので後回し」とされていた項目で、`git apply --cached` を使う
  - **対象は「上端に見えている行が属する hunk」**（`GitState::current_hunk_ordinal` = `hunks()` を `viewport.scroll` で二分探索）。`]`/`[` のジャンプ先とそのまま一致する定義にしてあり、ペインタイトルに `hunk k/n` を常に出して暗黙にしない。diff ペインにカーソル（行選択）を持ち込まずに対象を一意にするための選択で、行単位のステージは今もスコープ外
  - **表示形式に依らず同じ hunk を指す**: `hunks()` は inline / side-by-side / side+wrap で別々の行 index を返すが、hunk の並び順は生 diff と同じなので**序数だけを取り出して**生 diff 側 (`GitDiff::raw_hunks`) を引く。表示行 → 生 diff の対応表を表示形式ごとに持たない
  - **パッチは生 unified diff から組み立てる**（`GitState::current_hunk_patch`）。表示用の `Line` 列からは復元できない（`classify` がファイルヘッダを落とし、`text::normalize` がタブを空白へ展開する）ため、`GitDiff` が取得時の `raw` を持ち続ける。ファイルヘッダ（最初の `@@` より前）+ その hunk の生行を連結するだけで **hunk header の行番号は書き換えない** — `git apply` は文脈行を照合して適用位置を決めるので、先行する hunk が未適用でもオフセットを吸収する（`git add -p` と同じ作法）
  - **向きは `DiffBase` が決める**: Head/Unstaged は stage（`git apply --cached`）、Staged は unstage（`--reverse`。index vs HEAD の hunk を index から外すのが「取り消し」にあたる）。押す前にどちらになるかはステータスバーのヒントに出す
  - **untracked は拒否する**（`GitDiff::stageable`）。`git::file_diff` の `--no-index` フォールバックはヘッダに**絶対パス**を載せるため、そのまま apply すると repo 外へファイルを作ろうとする。ヘッダの `+++ b/<path>`（削除なら `--- a/<path>`）が期待する repo 相対パスと一致するときだけ true にし、判別できない形（git がクォートしたパス等）は false = 拒否側へ倒す。まとめ diff (`A`) 中も「ファイルヘッダを 1 つに決められない」ため拒否し、その間はタイトルの `hunk k/n` も出さない（押せば効くと読めてしまうため）
  - `Mode::Confirm` は挟まない（ツリー側の Space と同じく非破壊的でいつでも打ち消せる）。`STAGE_DEBOUNCE` はツリー側と**同じタイマーを共有**する（粒度が違うだけで「実行キー本体の連打で git が暴走する」問題は同じ）。EDIT レーンの未保存バッファに対するガードも要らない — `git apply --cached` は index だけを書き換え worktree のファイルに触らないため、discard/stash と違って編集内容と食い違う余地が無い
  - HEAD 基準では「その hunk の文脈行が既にステージ済み」だと index に対して適用できず git が失敗する。失敗理由をこちらで判別せず stderr をそのまま出し、**次の一手だけ添える**（`t` で unstaged 基準に切り替える）
- **word-level ハイライト（#29）**: `render_inline` が行の Vec を作る前に、hunk 内で「連続する削除ブロック → 直後の連続する追加ブロック」を検出し、**行数が一致する時だけ**先頭から 1 対 1 で対応付ける（ズレたペアより「対応付けない」方が読みやすいため、行数不一致・打ち切り超過は何もせず従来の全行色のまま）。文字単位の差分は `editor::diff::word_diff`（component/editor/diff.rs の LCS を `T: PartialEq` で汎用化し、行の LCS と共有）で計算し、双方の行で「共通部分に含まれない char range」を求める。gutter (span[0]) は不変のまま、content 側 (span[1] 以降) だけをその range で複数 span に割り、前景色はそのまま背景だけ濃くする。diff 行の先頭 1 文字は `+`/`-` マーカーなので char 単位比較の対象から外し、range をマーカー分 (+1) ずらして戻す。計算量は行の char 数（500 超で `word_diff` が None）と 1 hunk あたりの対応ペア数（200 超で以降のペアをスキップ）の 2 段で打ち切る。打ち切られた行は元々の単一 span のまま描画され、span[1..] を連結すると本文に戻る前提は崩れない。この word_ranges は `render_side_by_side` とも共有する（同じ classify 済み body・同じ index で引くだけなので、side-by-side でも変更文字の強調がそのまま出る）
- **side-by-side（#30、`v` で inline と切替）**: GIT レーンの単一ファイル diff のみ対応（LOG の複数ファイル diff は inline のまま）。`render_side_by_side` が classify 済みの body から**行の対応が取れた 2 本の `Vec<Line>`**（左 = 旧側行番号・右 = 新側行番号）を作るところまでを持ち、`component/gitlane/view.rs` がそれを `TextPane` に 2 回渡すだけ — **text_pane.rs には side-by-side 専用の分岐を足さない**。削除ブロック→追加ブロックのペア (word-level と同じ run 検出) を「大きい方の行数」に揃え、足りない側は gutter だけの空行 (`blank_row`) で埋める。gutter 幅・最長行幅 (hscroll のクランプに使う) は左右で別々に持つ (`SideDiff`)
  - **wrap との併用**: `Viewport` は 1 個 (scroll/hscroll は左右共有) だが、wrap 中に各カラムを独立に char 単位分割すると同じ論理行でも視覚行数がズレる。対策は「vp.scroll を常に論理行 index として使う」という既存の前提を保つために、**wrap 幅が分かる描画時 (毎フレーム) に `side_by_side_wrapped` で両カラムを事前に char 分割し、行ごとに視覚行数が少ない方へ空行を足して総行数 (=論理行数) を揃えてから**、TextPane には `vp.wrap = false` の一時コピーで渡す (Viewport を Copy にしたのはこのため)。これにより TextPane 自体は普通の非 wrap スライスをするだけで済み、ここでも text_pane.rs は変更していない。wrap 幅依存の計算なのでリサイズのたびに変わり、キャッシュせず毎フレーム作り直す (行数・hunk 位置だけ `GitState::side_wrap_cache` に書き戻して scroll クランプ・n/N に使う。viewport.height/width と同じ 描画→app の書き戻しパターン)
  - **幅不足の自動フォールバック**: `GitState::side_by_side` はユーザーの意図のトグル、`side_by_side_active` は「実際に描けるか」(各カラムが gutter 込み 40 桁以上あるか) を毎フレーム `viewport.width` から判定する関数。表示・スクロール・hscroll・hunk ジャンプの全てがこの `side_by_side_active` 1 箇所を参照するので、ペイン幅のドラッグリサイズで縮めても「見た目は inline なのに内部状態は side のまま」というズレが起きない。トグル自体は変えないので、幅を戻せば自動で side-by-side に復帰する
  - 状態は `w` と同じく `GitState` に持ち config には保存しない
- **diff 内検索（#31、`/` `n` `N`）**: `component/viewer/search.rs` の `search_matches`（`pub(crate)` に格上げ）と `SearchState` をそのまま再利用する。`GitState` は自前の `plain: Vec<String>` を持たず、検索のたびに `lines()`（今表示している inline 行）の各 `Line` を `span[1..]` 連結で文字列化してから渡す — word-level ハイライト (#29) で複数 span に分割された行でも連結すれば normalize 済みの content に戻るため、桁のズレは起きない。**キー衝突の解消**: hunk ジャンプは `]` `[` に一本化し、`n` `N` は検索の次候補/前候補に譲った（VIEW の検索と同じキー配置に揃える）。Search の確定先 (`Mode::Input { kind: Search }`) は `App::confirm_input`/`cancel_input`/`live_update_input` が `Lane::Git` かどうかで `viewer` と `GitState` のどちらを呼ぶか振り分ける（Goto は View の `:` からしか届かないので lane 分岐は不要）。**side-by-side 表示中は `/` 自体を出さない**（`on_git_key` が `side_by_side_active()` でガード）: 左右が独立ドキュメントで一意な行位置を持たず、`viewport.scroll` の意味が inline と食い違うため
- **全ファイルまとめ diff（#31、`A` でトグル）**: `git::diff_all` がファイル指定なしの 1 回の `git diff <base>` に untracked 分（`--no-index`、`file_diff` と同じ理由で Staged では連結しない）を連結し、`gitlane::render_commit`（LOG レーンの `git show` と共有、複数ファイル diff のレンダラを 2 箇所に複製しない）にそのまま通す。untracked 分は `-C root` の cwd 相対パスに変換してから `--no-index` に渡す — 絶対パスのままだと `render_commit` の `segment_label` がヘッダの `+++ b/<path>` から抜き出すファイル境界ラベルが長い絶対パスになってしまうため。**巨大 diff の打ち切り**は行数 20000 / バイト数 2MB のどちらか先に達した時点で `git::diff_all` が切り詰め、`bool` で呼び出し側に伝える。rescan 経由の背景再取得（`GitState::refresh`）では打ち切りを notice に出さない（500ms デバウンス毎にスパムしないため）。`A`/`t` の明示操作 (`on_git_key`) だけが notice を出す。ON にする瞬間だけ取得し直し、OFF に戻す時は取り直さない（`current` 側の単一ファイル diff は `all` と独立に保持したまま）。**ツリーでファイルを選び直すと `GitState::exit_all` で解除**する一方、`GitState::open` 自体は `showing_all` に触れない（rescan 経由の `refresh` も内部で `open` を呼ぶため、そこで解除すると背景更新のたびに `A` が勝手に外れてしまう）。side-by-side とは併用しない（`side_by_side_active` が `!showing_all` を先頭でチェックし、まとめ表示中は常に inline）。`AllDiff` は `Lane` enum (`clippy::large_enum_variant`) のサイズを抑えるため `Box` で持つ
- **sticky header の共有（#31、#40 の再利用）**: `render_commit` が返すファイル境界一覧 (`Vec<(usize, String)>`) の二分探索ロジック (`gitlane::sticky_label`) と、描画側のバンド強調・truncate (`widget/diff_boundary.rs`) を LOG (`component/log/view.rs`) と GIT のまとめ diff (`component/gitlane/view.rs`) の両方から呼ぶ 1 箇所に寄せた。sticky 行 1 行分の高さ確保は LOG と同じく「境界を持つか」だけで判定し scroll には依存させない（`Ctrl+d`/`Ctrl+u` のページ送り量がスクロール中に変わらないようにするため）

### コミット（app/commit.rs + shell/commit.rs）
- `c`（通常コミット）/ `C`（amend）は `Mode::Commit` を開く。開けるかどうかは都度判定するだけで、GIT レーンに滞在している必要はない（キールーティングは前節参照）
- **Esc は内容を破棄しない**。`App.commit_draft` / `App.amend_draft`（ともに `Option<String>`）に退避し、次に `c`/`C` を押した時に復元する。長文の途中の誤操作で消えるのが一番痛いという issue の要求そのもの。amend は既存メッセージのプリフィル（`git log -1 --format=%B`）があるため通常コミットとは別の下書きフィールドにする — 同じフィールドにすると「amend の下書き」を開いたつもりが直前の通常コミットの下書きで上書きされる、といった取り違えが起きる
- amend の確認 (`ConfirmAction::Amend`) を開く直前にも `amend_draft` へ退避する。確認をキャンセルしても Mode::Commit には戻らず `Mode::Normal` になる (`on_confirm_key` の設計) ため、退避しておかないとキャンセルで書きかけを失う。通常コミットは確認を経由しないのでこの退避は不要
- staged が空なら通常コミットは開かず notice を出す（`--allow-empty` は使わない）。amend は staged 空でも許可する（メッセージ修正の用途）。判定は `App::has_staged_changes`（`FileStatus.index` が `Some` のファイルが 1 つでもあるか）
- 未保存の EDIT バッファがある間はコミットしない、という issue の要求はガードとして明示的に書いてある（`open_commit` 冒頭）が、実際には `Lane::Edit` は印字キーを全て文字入力にして `c`/`C` をこの分岐まで届かせないため、現在のキールーティングでは到達しない防御的コードである
- pre-commit hook 失敗時、通常コミットは `Mode::Commit` に留まったままオーバーレイ内 (`error` フィールド) にエラーを出し、書きかけのメッセージを保って再試行できるようにする。amend は確認オーバーレイ経由で実行され、失敗時点で `mode` は既に `Mode::Normal` に戻っているため `App.notice` でエラーを出す（下書きは確認前の退避で既に保持済み）
- 成功後の再取得は stage/unstage と同じ `App::rescan`（r キーと同じ入口）に相乗りさせる。新しいタイマーは作らない
- **`git log --format=%B` の末尾改行は 2 個並ぶ**（git がコミット保存時にメッセージ末尾を改行 1 個に正規化し、`log --format` がさらにエントリ区切りの改行を足すため）。amend プリフィルで 1 個だけ剥がすと空行が編集バッファに残ってしまうバグを踏んだので、`git::last_commit_message` は末尾の改行を `trim_end_matches('\n')` で全部落とす
- ルーラー行（50/72 桁の目安）は `shell/commit.rs::ruler_line` が区切り線として出すだけで、入力を強制しない（issue の要求通り）
- カーソルは EditState と同じ発想で REVERSED スタイルの重ね書き。`shell/commit.rs` は `Paragraph::wrap` を使う数少ない例外 — TextPane が禁じているのはカーソル位置を外部 (click_at 等) から計算する必要があるためで、ここはカーソルが文字に貼り付いたスタイルとして流れるだけなので外部座標計算が要らず問題にならない

### 破棄 (discard) と stash（#25、app/git_ops.rs + git/write.rs）
- `X`（選択ファイル/ディレクトリの破棄）・`z`（stash push）は **GIT レーン限定**（`Lane::Git(_)` の間だけ、focus 別ディスパッチより前で拾う）。対象は Space のトグルと同じく `tree.selected` を見るので Focus::Tree/Viewer どちらでも同じ挙動になる
- `Z`（stash pop）だけは **GIT レーンに縛らない**（`log_available`＝git repo でありさえすれば、どのレーンからでも呼べる）。`z` で変更を全部退避すると `git_available` が false になり GIT レーンへ再入場できなくなるため、GIT レーン限定にすると「push した直後に pop で戻れない」事故になる。X の対象決定に必要な `tree.selected` を pop は必要としないので、レーンを問わず呼べても安全側が壊れない
- 3 つとも `Mode::Confirm` を経由する（`ConfirmAction::Discard { path, is_dir }` / `StashPush` / `StashPop`）。prompt は `\n` 区切りの複数行に対応させた（`shell/confirm.rs` が `prompt.lines()` を素直に複数 `Line` へ割る。単一行の既存呼び出しは互換のまま）。discard の prompt には対象パス・件数・untracked を含むかを出す
- 未保存の EDIT バッファに対する防御 (`App::refuse_if_edit_dirty`) は、現行のキー経路では実質到達しない（`Lane::Edit` は全ての印字キーを文字入力として奪うため X/z/Z がそこへ届かず、かつ Lane は同時に一つしか存在しない）。それでも issue の安全側の作法として明示的に置いている（belt and suspenders）。テストするなら「EDIT で未保存のまま Shift+Tab が拒否される」という既存の `cycle_lane` の挙動で事実上検証できる
- `git::discard_path`（tracked 分）: 通常は `git restore --source=HEAD --staged --worktree --`。**HEAD の無い初期 repo ではこれが必ず失敗する**上、`unstage_path` のような「`--source` を外すだけの再試行」は効かない（`--staged` を含む限り既定の source は常に HEAD で、明示指定を外しても変わらないため）。フォールバックは `--worktree` 単独（index を基準にでき HEAD を要求しない）で worktree を index に揃えたあと `git rm --cached`（`unstage_path` と同じ）で index から外す、という 2 コマンド構成にした。結果としてファイルは「HEAD の内容」には戻れず（存在しないため）untracked のまま残る — 破棄としては不完全だが、HEAD 未解決のエラーをそのまま見せるより安全側（誤ってファイルを消さない）に倒している。untracked になったファイルは再度 `X` を押せば untracked 側のパスで削除できる
- untracked 分の削除は git を使わず `std::fs::remove_file` で扱う。tracked/untracked の判定・分岐は `App` 側（`s.index == Some(StatusKind::Untracked)`）にあり、`git.rs` 側には持たせない（`git.rs` は個々の git コマンドのラッパーに徹し、「どのファイルが untracked か」の判定はステータス構造体を持つ App 側の責務のままにする）
- GIT レーンの diff は「開いていたファイルの内容そのものが変わった」場合に自動追従しない設計のままだと事故る: discard/stash 後に `App::rescan` → `GitState::refresh` が**同じ path** で diff を再取得するため、破棄でその path が `changed_paths()` から外れていても再取得を試み、`git::file_diff` の untracked `--no-index` フォールバックが「新規ファイルの全行追加」という誤った diff を出してしまう（clean な tracked ファイルの diff は空文字列になり、untracked フォールバックと区別できないため）。これを避けるため discard/stash 実行後は `App::refresh_git_diff_selection` を呼び、`tree.selected_or_first_file()` が指す**ツリー側の新しい選択**へ diff を明示的に向け直す（`enter_git` の初期化と同じ `GitState::open` 呼び出し）。通常の j/k 移動で diff を追従させない設計（キーリピートで git を連打しないため）とは理由が異なる点に注意
- discard/stash 実行後の VIEW キャッシュ更新は保存時と同じ `Viewer::reload` 経由。discard は対象パスが明確なので `open_path == path`（ディレクトリなら `starts_with`）の時だけ reload する一方、stash は working tree 全体に影響し対象を事前に絞れないため、現在表示中のファイルを無条件で reload する

### ブランチ一覧オーバーレイ（component/branch/ + app/branch_ops.rs）
- `b` は `c`/`C` と同じ位置（グローバルキー相当）で `Mode::Branch` を開く。GIT レーンにいる必要はなく、可否 (`App::branch_available`) は LOG と同じ「git repo でありさえすればよい」基準（変更の有無を問わない）
- 一覧は `git for-each-ref` を 1 回叩くだけ（`git::branches`）。issue の指定フォーマットに **フルの refname を先頭へ追加**している — `refname:short` だけではローカル/リモートの判別ができない（両方とも単なる短縮名で、リモート名にたまたま `/` が入っていても区別がつかない）ため、`refs/remotes/` プレフィックスを見て確実に判定する。`origin/HEAD` のような symbolic ref は `full.ends_with("/HEAD")` で除く
- 絞り込みは新しいマッチャを書かず `component/finder/mod.rs::fuzzy_match` を再利用する（`pub(crate)` にして公開）。`BranchState`（component/branch/mod.rs）は Finder と同じ「候補 + クエリ + 選択位置」の骨格だが、current マーク・local/remote・upstream・相対日時・件名という Finder の `candidate: String` では表現できない付随情報を持つため専用の型にした。表示は「ローカル/リモートで分ける」という issue の要求を、スコアでソート → local/remote で安定ソート（グループ内の順序はスコア順を保つ）という 2 段階で満たす。true のグループ分けヘッダ行は List のインデックス対応が壊れる（ヘッダ行がある分だけ選択インデックスがずれる）ため避け、行ごとに `local `/`remote` タグと色で区別するだけに留めた
- current マークは `App.branch_status`（後述）の名前と突き合わせるだけで、オーバーレイを開くたびに新しく git を叩き直さない。detached HEAD では current が常に false になる（一致する名前が無いため自然にそうなる）
- `Enter` = 選択行が local なら `git switch <name>`、remote なら `git switch --track <remote>/<name>`（`git::switch_branch` / `git::switch_track_branch`）。`Ctrl+n` はクエリがどのローカルブランチ名とも一致しない時だけ `git switch -c <query>`（`BranchState::matches_existing_local`）— 一致する間は同名衝突エラーを git に出させず事前に notice で理由を示し、オーバーレイは開いたまま再入力させる
- 未保存 EDIT バッファのガードは `open_branch` 冒頭にあるが、コミットの `open_commit` と全く同じ理由で実際には到達しない防御的コードである（`Lane::Edit` が印字キーを全て文字入力にするため `b` はそこまで届かない）
- 切替後は stage/unstage・コミットと同じ `App::rescan` に相乗りさせる（専用の同期パスを作らない）。**開いていたファイルが切替先に存在しない場合**は `Viewer::close`（今回追加した薄いメソッド。cache・履歴には触れず `current` だけ落とす）で右ペインを空にし、notice にその旨を付記する。存在判定は `rescan()` 呼び出し前（＝実際の checkout 後）に `open.path.exists()` で行う
- 成功・失敗どちらも `Mode::Normal` に戻して `App.notice` で結果を見せる（`finish_branch_action` 1 箇所に集約）。dirty で checkout が失敗した時は **git の stderr をそのまま** notice に出し、fv 側で stash 等の自動対応はしない（issue の明示的な要求）
- ステータスバーの現在ブランチ + ahead/behind (`App.branch_status: Option<git::BranchStatus>`) は `git::branch_status`（`rev-parse --abbrev-ref HEAD` + `rev-list --left-right --count @{upstream}...HEAD`）を起動時と `App::rescan`（500ms デバウンス）に相乗りさせて取得する。描画のたびには叩かない。detached HEAD は `abbrev-ref` が "HEAD" を返すことで判定し、短縮 SHA を代わりに name へ入れる。upstream 未設定は `@{upstream}` の解決失敗（非 0 exit）を異常系にせず `has_upstream: false` で吸収し、ahead/behind は 0 のまま何も表示しない。この表示は Lane・Mode を問わず常時出す（GIT レーン限定の `DiffBase` 表示等とは別物）

### LOG レーン（component/log/）
- 一覧は `git log --format=%H%x00%h%x00%an%x00%ar%x00%s -z -n <limit> --skip=<skip>` を `git.rs::log` で自前パース（porcelain -z と同じ流儀）。初回 200 件、選択が末尾に到達したら同じ関数を `--skip` を進めて呼び直す（ページング）。取得件数が要求件数未満だった時点で `exhausted` を立て、以後は呼ばない（held-key で連打しても追加の `git log` は末尾到達時に高々 1 回）
- コミットが1件も無い repo は `git log` 自体が失敗するが、`git.rs::log` はこれを空 Vec に潰して返す（エラーではなく「0 件」という正常系）。`LogState`/一覧描画のどちらも空を前提に組んであるので panic しない
- **左ペインはツリーではなくコミット一覧**（`LogState.commits`）。GIT の「ツリーを絞り込む」方式とは異なり、ツリー自体を使わない別の描画パス（`component/log/view.rs::draw_log_list`）を持つ
- 一覧の j/k は選択移動のみで diff を開かない（GIT のツリーと同じ理由）。**Enter/l/クリックでのみ** `LogState::open_selected` を呼び `git show` を実行する。開いた diff は `open_index` で選択中カーソルと別に持つ（j/k で `selected` が進んでも `open_index` はそのまま残る）
- 右ペインは `git show --no-color <sha>` を `gitlane::render_commit` で組み替えたもの。既存の GIT レーン（単一ファイル）の `render_inline` はそのまま温存し、`build_body`（1 行単位の組み立て: classify → 色分け → gutter 付与）を共有ヘルパーへ切り出して両方から呼ぶ形にしてある（#23/#29 と同時進行だったため、`render_inline` 自体への変更を最小化する意図）。LOG は side-by-side (#30) のスコープ外なので `render_commit` は inline 専用のまま
- 複数ファイル diff は `diff --git ` 行を境界に分割し、ファイルごとに見出し行（rename は `old → new`、新規/削除は `(new)`/`(deleted)` を付記）を挟んで連結する。**gutter 幅は全ファイル共通の 1 つに揃える**（ファイルごとに違う幅だと `TextPane` の wrap 計算・continuation 行の pad 幅がずれるため。単一ファイルの `render_inline` はそのファイルだけの幅で良いが、`render_commit` は全体の最大行番号から 1 つの幅を出してから `build_body` を呼ぶ）
- コミットメッセージ部分（`diff --git` より前の行）は gutter を空欄にしたまま別の色で出す。行番号の概念が無いコンテンツでも「span[0] = gutter 固定」の桁インバリアントは崩さない
- **マージコミットの表示方針**: `git show` は既定でマージコミットの差分を出さない。全親差分 (`-m`) は本文が膨らみすぎて読みにくいため採用せず、**最初の親との diff のみ**を明示的に組み立てて見せる（`git show --quiet` でメッセージ部分、`git diff <sha>^1 <sha>` で diff 部分を取得し連結）。あわせて `(merge commit: diff against first parent)` の注記行を挟み、暗黙に一部の差分だけを見せていることが分かるようにする
- Viewport は VIEW/EDIT・GIT の diff のどちらとも別に持つ（`LogState.viewport`）。別ドキュメントなので位置を共有する意味が無く、他レーンの読み位置も壊さないのは GIT の diff Viewport と同じ理由
- `.git` 配下は watch.rs のフィルタで最初から監視対象外（`.` 始まり成分は除外）なので、コミット追加を検知して一覧を自動更新する経路は無い。GIT のような 500ms デバウンス再取得への相乗りはしていない（コミット履歴の閲覧は「その時点のスナップショットを読む」用途と割り切り、動くリポジトリで追従させたい場合は一旦 LOG を出入りし直す想定）。repo 自体が消えた場合だけは `App::rescan` が VIEW へフォールバックさせる
- 絞り込み（Ctrl+f 等でファイル単位のログに切り替える機能）は本 issue のスコープ外として見送った。実装するなら `git log -- <path>` を `git.rs::log` に path 引数を足す形で追加できる
- **複数ファイル diff の sticky header（#40）**: `render_commit` の戻り値に「ファイル見出し行の index → ラベル」の `Vec<(usize, String)>` を追加で持たせている（既存 4 要素の意味・生成ロジックには手を入れない。#23/#30 と `component/gitlane/` を共有するための衝突回避）。`LogState::sticky_label` が `viewport.scroll` 以下で最大の index を `partition_point` で二分探索し、該当ファイルのラベルを返す（scroll は wrap 中でも常に論理行 index なので、折返し・hunk ジャンプのどちらでも別扱いが要らない）。描画は `component/log/view.rs::draw_log_diff` が担当し、`TextPane` には sticky 用の分岐を足さない。sticky 行 1 行分は `TextPane` に渡す高さを事前に減らして確保する。**減らすかどうかは scroll ではなく「このコミットの diff にファイル境界が 1 つでもあるか」で決める** — scroll 依存にすると commit メッセージ部分と本文とで高さが変わり、`Ctrl+d`/`Ctrl+u` のページ送り量がスクロール中に変化してしまう（`viewport.height` の書き戻しは減らした後の値を使う、という他レーンと同じ制約）。長いパスは先頭のディレクトリ階層から `…/` 付きで落としていき、それでも収まらなければファイル名側を char 単位で切る（末尾優先）。流れる側の境界強化は `render_commit` のヘッダ行が付ける固定背景色を目印に、描画側で右側をペイン幅まで同じ色で埋めて全幅の帯にするだけに留めている（gitlane 側の行組み立てには触れない）

### ビューアの範囲選択とコピー（component/viewer/selection.rs + clipboard.rs）
マウスキャプチャを有効にしている（main.rs の `EnableMouseCapture`）間は端末ネイティブのドラッグ選択が奪われるため、「読んでいるコードを AI に貼る」が fv の中だけでは完結しなかった。VIEW レーンに自前の範囲選択とコピーを持たせてこれを埋める。
- **入力は 2 経路**: マウスの押下 → 移動 → 離す（char 単位）と、`v` の行単位選択（`j`/`k`・`Ctrl+d`/`u`・`gg`/`G` で伸縮）。`y` でコピー、`Y` で開いているファイル全体、`Esc` で解除。GIT/LOG の右ペインは別ドキュメント（diff）なので対象外にし、`Lane::View` だけに閉じている（`App::begin_viewer_selection`）
- **閲覧にはカーソルが無い**（EditState を持つのは EDIT だけ）ので、`Selection` 自身が anchor/head を持つ唯一の位置情報になる。座標は plain（タブ展開済み）の char インデックスで、検索マッチと同じ「描画桁」座標の既存のインバリアントにそのまま乗る — 検索マッチと同じやり方で背景色を重ねられる（`text_pane::highlight_selection`）
- **コピーする中身だけは raw から取り出す**。plain のままだとタブが空白 4 個に化けて貼り付け先のインデントが壊れるので、`text::char_col_at` で桁を raw の char 座標へ戻してから切り出す。この「plain で位置を持ち raw で中身を取る」変換が `Selection::text` の唯一の役割
- **クリック座標 → 論理行/表示桁の変換は `Viewport::locate` に一本化**した（EDIT の `click_at` も同じ関数を通す）。折返し中の視覚行の辿り方を描画（text_pane）・カーソル追従（ensure_visible）・クリック座標の 3 者で揃えるという既存のインバリアントを、コードとしても 1 箇所にしたもの
- **選択中は j/k の意味を linewise の時だけ変える**。`v` で始めた行単位選択の間は移動キーが伸縮に化け（vim の visual mode 相当）、マウスで作った char 単位選択の間は移動キーが通常どおり画面だけを動かす — ドラッグし終えてからスクロールして全体を確かめ、それから `y` を押せるようにするため
- **選択の右端は行なりでラギッドなまま**にしてある。ペイン幅まで空白で埋めれば矩形に見えるが、埋めた空白ぶん wrap の折返し位置と hscroll の桁がずれる（text_pane の行加工順が前提にしている「span[1..] を連結すると本文に戻る」も崩れる）ため、見た目より桁の整合を優先した
- **コピー手段は `clipboard.rs`**: `pbcopy`/`wl-copy`/`xclip`/`xsel`/`clip.exe` を順に試し、どれも無ければ OSC 52 で端末に投げる。git と同じく CLI 呼び出しで済ませ、クリップボードのクレートも base64 のクレートも足さない（`base64` 関数は 20 行）。ローカルのコマンドを先に試すのは OSC 52 が端末側の許可設定に左右され、拒否されても何も返ってこない（無音で失敗する）ため。X/Wayland が無い環境で `xclip`/`wl-copy` を起動すると接続先を探しに行って TUI が止まるので、`DISPLAY`/`WAYLAND_DISPLAY` が見えている時だけ試す
- **使った手段は notice に出す**（`copied 12 lines / 238 chars (osc52)`）。OSC 52 の無音の失敗を切り分ける手掛かりがこれしか無いため。OSC 52 は端末そのものへの指示なので alternate screen の中身を汚さず、ratatui と同じ stdout へ書いて flush するだけでよい（フレームの描画とは独立）
- **選択はファイルを開き直した時点で捨てる**（`set_current`/`reload`/`close`）。行が入れ替わった後の桁を指したままにしないため。逆に `y` の後は残す — 同じ範囲をもう一度取り直せる方が使いやすく、消したい時は `Esc` がある

### インライン編集（component/editor/）
- `Lane::Edit(EditState)` が編集状態（バッファ・カーソル・undo）を所有し、「編集中なのに状態が無い」を型で排除する（Finder と同じパターン）
- `EditBuffer` は disk から**生テキストを独立ロード**する。viewer の `plain` はタブ展開済みで保存に使えない。CRLF・末尾改行を記憶し `to_text()` で復元（保存でファイルを壊さないための核）。undo/redo は Insert/Delete 2 種の op の逆適用で、連続タイピングは coalesce（カーソル移動・改行・保存・ペーストで区切る）
- カーソルは端末カーソルでなく REVERSED スタイル重ね（全角・タブの画面幅計算を回避）。検索ハイライトと同時には使わない（TextPane の search と cursor は排他）
- 編集は `HighlightCache::invalidate_from(変更行)` を呼ぶだけで、実際に色を付け直すのは次の描画で画面に映る行のみ（「閲覧と編集の関係」節）。キーストローク毎にファイル全体を舐めないので、閲覧側と別の「編集中はプレーン表示に落とす」閾値も要らなくなった。`Content` cache は編集中は使わず、保存時の `viewer.reload()` で更新する
- `EditBuffer::source()` は最終行にも改行がある扱いで固定する（`str::lines` の行数を `lines.len()` に一致させ、末尾空行の描画欠けを防ぐ）。閲覧側（`TextDoc::source`）は元ファイルの末尾改行の有無をそのまま伝える
- 変更行マーク `▎` は編集中も出る。ただし viewer と違い**未保存バッファのライブ diff**: 編集開始時に `git.rs::baseline_lines`（HEAD → 初期 repo は index。changed_lines と同じ基準）を 1 回取得し、以後は編集の度に component/editor/diff.rs（prefix/suffix トリム + LCS 自前実装）で再計算する。git CLI をキーストローク毎に呼ばない
- 既知の制約: 外部変更との競合は last-write-wins（保存が上書きする）。非 UTF-8・10MB 超は編集不可（`e` が no-op）

### 一時通知（App::notice と EditState.notice）
`App.notice: Option<(String, Instant, bool)>` は全レーン共通の一時通知で、GIT の書き込み結果などレーンを離れても見せたいメッセージに使う。`EditState.notice`（EDIT レーン専用・保存エラーや discard 確認に使用）とは役割を分けたまま両方残す — EditState 側は「Viewport だけを借りる」依存範囲の制約があり、App 全体の状態を持たせると設計が崩れるため統合しない。期限切れは `on_tick` でのみ判定し（`watcher` が無い環境でも on_tick 冒頭で判定するので消えなくなることはない）、再描画のたびにタイマーを触らない点は他のデバウンス系の方針と揃えている。ステータスバーでは `Mode::Confirm` の prompt → `App.notice` → レーン別ヒントの優先順で 1 行に出す

### git 連携（git.rs）
git2 クレートは使わず CLI を `GIT_OPTIONAL_LOCKS=0` 付きで実行。porcelain -z の rename は `XY new\0old\0` の 2 パス形式。`git diff HEAD` は HEAD 無し repo で fail するため素の diff にフォールバックする。全失敗を Option で吸収し panic しない。
- 読み取り (`run_git`) と書き込み (`run_git_write`) は別関数。`run_git` の `GIT_OPTIONAL_LOCKS=0` は読み取り専用が前提の意図的な設定で、書き込みにそのまま使うと `git add` 等が index lock を取れず壊れうるため統一しない。`run_git_write` は `GIT_TERMINAL_PROMPT=0` を付け、認証待ちで TUI がハングするのを防ぐ（fetch/push 等のリモート操作で効いてくる）。結果は `GitOutcome { ok, message }` で返し、失敗を `Option` にせず `ok: false` に潰すのは `run_git` と同じ「呼び出し側を単純にする」方針を書き込み側にも踏襲したもの
- 書き込み成功後の再取得は専用パスを新設せず `App::rescan`（r キーと同じ入口）に相乗りさせる。GIT の 500ms デバウンスと同じ考え方を書き込み後の同期にも適用している
- `unstage_path` は `git restore --staged` が失敗したら理由を判別せず `git rm --cached` にフォールバックする。`changed_lines`/`baseline_lines`/`diff_text` の「まず試す → だめなら別コマンド」という既存方針をそのまま書き込み系にも踏襲したもので、失敗理由を HEAD の有無で個別判定していない
- `commit` はメッセージを、`apply_cached`（hunk 単位ステージ）はパッチを、どちらも引数ではなく **stdin から渡す**（`-F -` / `apply -`。エスケープ・コマンドライン長の問題を避けるため）。`run_git_write` は `Command::output()` で完結できるが、stdin を渡すには `spawn` → `stdin.take()` に書き込んで drop（EOF 送出）→ `wait_with_output` という別の実行経路が要るため `run_git_write` は流用せず `run_git_stdin` を 1 つだけ持ち 2 者で共有する。`apply_cached` に `--whitespace=nowarn` を付けるのは、既存の空白エラーを含む行で git がパッチを弾く（＝画面に出ている hunk なのに stage できない）のを避けるため — fv は diff をそのまま index へ移すだけで内容の整形はしない。成功時の短縮 SHA は commit の stdout（amend やルートコミットで書式が揺れる）ではなく `rev-parse --short HEAD` を別途叩いて確実な形を取る。stderr は pre-commit hook が複数行出すことがあるので、先頭の非空行 + 複数行あれば "…" を付ける専用の要約関数 (`stderr_summary`) を使う（`run_git_write` 共通の `first_line` とは仕様が違うため分けた）

### 非同期ジョブの基盤（job.rs）とリモート操作（f/p/P、#27）
- ネットワークを伴う fetch/pull/push は同期実行だと TUI が固まるため、`job.rs` に `std::thread::spawn` + `mpsc::channel` の薄いラッパー (`job::spawn`) を用意した。**git 専用にせず汎用にしてある**（GitHub 連携 (#33/#34) の issue/PR 取得も将来ここに乗る想定のため、git.rs には置かない）。結果の受け取りは専用タイマーを作らず既存の `App::on_tick`（`event::poll(100ms)` のたびに呼ばれる）で `try_recv` を drain するだけにし、イベントループの構造（main.rs 節）をそのまま使う
- `Receiver` 側 (App) がアプリ終了で先に破棄されても `tx.send` は Err を返すだけで panic しない（mpsc の仕様通りで、`spawn` 側は戻り値を握り潰すだけで良い）。スレッドを待たずに終了できるので `main::restore_terminal` を遅らせない
- 実行中のジョブは `App.pending_remote_job`（非 pub、`git::RemoteJobKind` + 開始時点の ahead/behind スナップショットを持つ）で表し、`App::start_remote_job` がジョブ起動の唯一の入口。**実行中は新しいジョブを一切受け付けない**（fetch/pull/push は全て `.git` を触るため、「同じジョブの二重起動防止」を「別ジョブとの直列化」に一般化した方が安全側で、実装も単純になる）。ステータスバーには `App::running_remote_job()` を通してのみ見せる
- 完了メッセージ（例: `push → origin/main (2 commits)`）は完了時点の `branch_status`（rescan 後）ではなく**ジョブ開始時点のスナップショット**（ahead/behind/upstream 有無）を使って組み立てる。rescan で ahead/behind が上書きされた後だと push 前のコミット数が分からなくなるため
- `git::run_git_remote`（fetch/pull/push 専用）は `run_git_write` の `GIT_TERMINAL_PROMPT=0` に加え、`GIT_ASKPASS`/`SSH_ASKPASS` を空文字にし `SSH_ASKPASS_REQUIRE=never` を付ける。認証プロンプトで裏のスレッドが無限に待つのが最悪の挙動なので、GUI askpass 起動経路も含めて確実に潰し、認証が必要なら即失敗させて notice に出す
- 失敗時のメッセージ抽出は `run_git_write` 共通の `first_line`（stderr 先頭の非空行）ではなく専用の `remote_error_line` を使う。`git pull --ff-only` の失敗は stderr に fetch の進捗行 (`From ...`) や `hint:` 行が本当の失敗理由より先に出るため、先頭行だと `fatal: Not possible to fast-forward` が隠れてしまう。`fatal:`/`error:` で始まる行を優先して拾い、無ければ従来通り先頭の非空行にフォールバックする
- `P`（push）だけ `Mode::Confirm`（`ConfirmAction::Push`）を経由させる。fetch/pull はローカルを (ff の範囲でしか) 変えないが push はリモートの履歴・ブランチ構成を変えるため。upstream が無ければ確認オーバーレイの時点で `--set-upstream origin <branch>` になることを prompt に出す。未保存の EDIT バッファは拒否まではせず prompt に警告行を足すだけ（issue の要求）だが、`open_commit`/`open_branch` と同じ理由で `Lane::Edit` が印字キーを全て文字入力にするため型上ここへは実際には来ない（belt and suspenders）
- 完了後の反映は他の書き込み系操作と同じ `App::rescan` に相乗りさせる（専用の同期パスを作らない）。ahead/behind・GIT レーンの diff・ツリーの status 表示が一度に揃う

### UI プレビュー（preview/）
「実装 → 保存 → 見た目を確認」を TUI の起動・操作なしで回すための開発用の入口（`cargo preview <scene>...`）。**dev 専用 feature `preview`（既定 off）**で、製品ビルドには一切入らない — `mod preview` から CLI の引数解釈（`preview::Options::take_flag`）・ヘルプ行まで全て `#[cfg(feature = "preview")]` で落とし、main.rs 側にプレビュー固有の分岐を残さない（feature off のビルドで `--preview` は unknown option になる）。`.cargo/config.toml` の alias で `--features` を毎回書かずに済ませている。ratatui の `TestBackend`（メモリ上の Buffer）へ `shell::draw` をそのまま通し、Buffer を ANSI 付きの文字列に落として stdout に出す。
- **描画側にプレビュー専用の分岐を一切足さない**。プレビューにだけ都合の良い経路を作ると「プレビューでは直っているのに実物が直っていない」が起きるため、`component/*/view.rs` 以下は 1 行も変えずに済む形（TestBackend + 既存の `shell::draw`）を選んでいる。同じ理由でシーンの状態は原則**キー列**（`preview/keys.rs` の `"<S-Tab>"` 等の DSL）で組み立てる — App の内部を直接いじると `app/keys.rs` の優先順位を通らない状態が描けてしまい、実機で再現できない絵になる
- 描画は「**描画 → setup → 描画**」の 2 回。`viewport.height` やペインの Rect は 描画側が App へ書き戻す構造（「描画は自前スライス」節）なので、1 回目で実測値を入れてからキー列を流さないと `Ctrl+d` のような高さ依存の操作がシーンごとにぶれる
- シーンが見るのは**必ず合成リポジトリ**（`preview/fixture.rs` が `$TMPDIR/fv-preview-fixture` に作り直す）。実プロジェクトを開くと「今の作業状態」で絵が変わって差分で比べられないため。staged / unstaged / untracked / 削除の 4 種と 3 コミットが常に揃い、コミット日時は実行時刻からの固定オフセット（相対日時の表示が毎回同じ見え方になる）。固定パスを消す以上、目印ファイル（`.fv-preview-fixture`）が無いディレクトリは消さない
- **利用者の config を書き換えない**。プレビューのキー列には `w`（折返し）のように `persist_config` を呼ぶものがあるため、`XDG_CONFIG_HOME` を使い捨てディレクトリへ差し替えてから App を作る（`isolate_config`、スレッドを 1 つも起こしていない時点で `set_var` する）
- **GitHub モードは全シーンで有効側に倒す**（`draw_scene` が `github_enabled`/`github_available` を直接立てる）。タブバーは Workspace を問わず全ての画面に乗るので、実機で GitHub モードを使っている時の見え方に揃える。`App::new` に `--github` を渡さないのは `github::check_available`（gh の有無・認証・リモートの判定）を走らせないため — 実環境への問い合わせで絵が変わってはいけないので、判定済みの結果だけを差し込む。タブの切替自体はキー（`<A-2>`/`<A-3>`）で行い、`workspace` を直接書き換えない。issues/PR の**中身**だけはキーで作れない（gh の応答が要る）ので、`begin_list_fetch` + `poll` に自前の `Receiver` を流し込む形で公開 API 越しに注入する
- 全角文字の桁送りだけは `preview/render.rs` に近似の幅計算を持つ（ratatui は全角の 2 セル目を空白へ reset するだけなので、そのまま出すと空白が 1 つ挟まる）。**これはプレビュー出力を端末の桁送りに合わせるためだけのもので、アプリ本体の桁計算（text.rs が唯一の定義）には使わない**
- **スナップショット = スクリーンショット（`--update-snapshots` / preview/snapshot.rs + preview/svg.rs）**: 全シーンを `docs/preview/<scene>.svg` に焼き、CI が描き直してコミット済みの画像と突き合わせる（比較器は Rust 側に持たず git に任せる）。**テキストのスナップショットは廃止した** — 同じ画面を txt と画像の 2 系統で持つ意味が無く、UI の差分は GitHub が SVG を描画して見せてくれる（Files changed の画像比較・PR コメントの before/after）。**差分があっても PR は落とさない** — 差分そのものは PR コメント（`scripts/ci-ui-screenshot-comment.sh`、シーンごとに `<details>` で畳んで **before | diff | after** を 1 行に並べる）で見せるので、赤い × は「見せる」目的に何も足さず作者に更新コミットを強いるだけになるため。コミット済み画像とのズレは main への push 時の自動追従コミットで解消する（PR で更新し忘れても、マージ後の main は必ず現在の描画と一致する。その間 PR コメントの "after" だけが実物とずれるので、食い違っている事実はコメント冒頭に出す）。代償として描画が非決定になった場合に「毎回 main へ追従コミットが積まれる」形でしか気づけない（マスクのユニットテストと、履歴に残る chore コミットの目立ちで足りるという判断）。**同じ画像を README も参照する**ので、手で撮ったスクリーンショットが実装から取り残されることが構造的に起きない
  - **PR コメントの真ん中の diff は shotdiff（`--diff-only`）**（https://github.com/Matuyuhi/shotdiff 、release のバイナリを CI で 1 つ落として使う。バージョンは固定する）。変わった画素だけをピンクに塗った 1 枚で、全画面を目で見比べなくても「どこが変わったか」だけが浮かぶ。**diff と after はその実行で描き直した実物**を使う（コミット済みの画像ではない）— 作者が `--update-snapshots` を忘れていてもレビュアーには現在の描画が見えるようにするため。どちらもリポジトリに無いファイルなので、置き場として履歴を持たない orphan ブランチ `ci-ui-diff`（`pr-<番号>/` 配下）へ push し、**そのコミット SHA を URL に使う** — ブランチ名で参照すると GitHub の画像プロキシ（camo）が古い絵をキャッシュし続ける。生成物しか置かないので毎回 1 コミットに潰して force push する（PNG が履歴に積み上がってリポジトリが太るのを防ぐ。同時に走った別 PR のぶんを上書きしうるが、次の実行で戻るだけなので許容する）。fork からの PR はトークンが読み取り専用で push できないので、その時だけ「コミット済みの画像」を after に使い diff の列を落とす
  - shotdiff（resvg）は SVG の文字を**システムのフォント**で描くので、CI に日本語フォントが無いと diff パネルの日本語が豆腐になる（before/after は閲覧者のブラウザが描くので影響しない）。`fonts-noto-cjk` を入れるが、入らなくても絵自体は出るので CI は落とさない
  - **マスクは Buffer の上でかける（`snapshot::mask`）**。実行のたびに変わるのはコミット SHA と `Date:` の絶対日時だけなので、そこだけ**桁数を保ったまま**伏せる（桁がずれると罫線が崩れ、README に載る画面としても読めなくなる）。文字列に落としてから伏せるのではなくセルを直接書き換えるのは、SVG が「どのセルが何色か」まで焼くため。日付は 1 文字ずつ潰すだけでは足りない — 日にちの桁（1 → 10）で長さが変わり後続が全部ずれるので、タイムゾーン `+0900` の終端までを「固定文字列 + 空白詰め」で置き換える。さらに**日付の桁から行末までスタイルも 1 つに潰す**（`flatten_style`）: 文字を固定幅に詰め直しても、元の日付の末尾で切れていたスタイルの境界はその日の長さのまま残り、SVG の run の切れ目が 1 桁ずれる。git の相対日時（`3 days ago`）は gettext の翻訳対象なので `LC_ALL=C` も `isolate_env` で固定する（日本語ロケールの手元と C ロケールの CI で食い違わせない）
  - **PNG にしない理由は 3 つ**: ラスタ化には依存（フォントを読むクレート）が要る / CI のランナーに日本語フォントが無く合成リポジトリの日本語が豆腐になる（SVG なら描画は閲覧者のブラウザ側）/ テキストなので git の履歴に置け、差分が行として出る
  - **桁の整合は 1 文字ずつ `x` を書いて取る**（`svg::glyphs`）。閲覧環境の等幅フォントは送り幅が違う（0.55em〜0.6em）ので run の先頭だけ置くと行の右へ行くほどずれ、`textLength` で run 幅を宣言すると字間・字形が引き伸ばされて読めなくなる（セルの格子に 1 文字ずつ載せるのは端末そのものの振る舞い）。**空白のセルは本文から落とす** — 位置決めは後続の文字が自分の `x` を持つので要らず、こうすると XML の空白の扱いに一切依存しなくなる（`xml:space="preserve"` は SVG2 で非推奨で、実際に Chromium では効かず字送りが 1 文字ずつずれた）。`CELL_W` はフォントの送り幅 0.6em ちょうどに合わせる（狭いと罫線 `─ │ ┌` が繋がらず枠が破線に見える。font-size 15 なら 9 で割り切れ、座標が全て整数になってファイルも小さい）。`Color::Reset`（端末に任せる意味）とテーマを持たない ANSI 16 色はここで具体的な RGB に決め打つしかないので、既定テーマ (base16-ocean.dark) の隣で浮かない値を選んである
  - stdout（`cargo preview <scene>`）は従来どおり ANSI 付きのテキストのまま（`render::buffer_lines` / `card`）。手元では端末にそのまま出すのが一番速く、マスクもかけない（本物の値のままの方が読める）

### 速度チェック（`cargo perf` + `.github/workflows/perf.yml`）
「AI が書いたコードをその場で手直しする」のが用途なので、**1 打鍵ぶんのコスト**が壊れていないかを見る。`src/preview/perf.rs` が preview と同じ dev 専用 feature の別入口として入っていて、`TestBackend` に `shell::draw` をそのまま通す（計測専用の描画経路は作らない — そこを分けると「ベンチだけ速い」が起きる）。状態もキー列で組み立てるので、`app/keys.rs` の優先順位を通らない経路を測ってしまうこともない。
- 計測は 4 ケース: `open`（20000 行のファイルを開いて 1 フレーム）/ `type`（EDIT で 1 文字ずつ 200 回）/ `scroll-down`・`scroll-up`（1 行ずつ 300 回）。どれも `on_key` → `draw` の 1 サイクルを 1 op と数え、`REPEATS` 回の**最小値**を採る（遅い側の外れ値は必ず外乱なので、速い方がその実装の実力に近い）
- 対象は固定の合成ファイル（`$TMPDIR/fv-perf-fixture`）。実プロジェクトを測ると「その時の作業状態」で数字が動いて 2 点間の比較にならない。git repo にはしない（git status の時間が混ざるうえ、測りたいのは描画のコスト）
- **pty 経由で実際に端末を起動する測り方は採らない**。起動待ちの sleep と端末の応答が支配的になってノイズが 30% ほど乗り、2 割の悪化が読めなかった。1 プロセスで完結する今の形は run 間のばらつきが数 % に収まる
- CI は `perf` ラベルの付いた PR でだけ回る（base 側もビルドして 2 回測るので毎 PR には重い）。`scripts/ci-perf.sh` が base と head を**同じ runner で連続して**測り、比を PR コメントに出す。GitHub の runner は同居ジョブで絶対値がぶれるため、単独の数字には意味を持たせない
- base 側のビルドは `git worktree` で別ディレクトリに出す。同じ作業ツリーを checkout し直すと**実行中のスクリプト自身が書き換わる**（base にまだこの計測が無い場合は消える）。`target/` は共有して依存のビルド成果物を使い回す
- **base にこの計測が無い/古い場合は、head 側の計測コードだけを base のツリーへ移植して測り直す**（`git checkout <head> -- src/preview`）。アプリのコードは base のまま、ものさしだけ head に揃えることで比較が成立する — これを追加した PR でも比較が出せるし、計測自体を変える PR で「base と head が違うものさしを見る」のも防げる。base が古すぎて head の `preview/` がコンパイルできない時だけ head の値だけ出す
- **遅くなっていても PR は落とさない**。UI スナップショット差分と同じ方針で、閾値（20%）を超えた時だけ `::warning::` を出す

## スタイル

- コメントは Why のみ・日本語。What の説明やコード写経コメントは書かない
- 再描画のコストを画面の大きさより上に持ち上げない。文書全体に比例する処理（全行の再ハイライト・全行分の `Line` 組み立て）を描画パスやキー処理に足さないこと。テキストは `HighlightCache`（可視範囲＋checkpoint）、ツリーは `component/tree/view.rs::visible_window` が既にこれを守っている
