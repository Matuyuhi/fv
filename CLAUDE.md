# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## 概要

fv は TUI コードビューア + インライン編集 + 変更レビュー（ratatui + crossterm + syntect + ignore + notify）。用途は「AI が書いたコードをその場で読んで手直しする」で、閲覧・編集（挿入・削除・undo/redo・ペースト・保存）・GIT レーン（変更ファイル絞り込み・diff・hunk/行単位の stage・コミット・discard/stash・ブランチ切替・fetch/pull/push）・コミット一覧・GitHub の issues/PR タブまで実装済み。VSCode 級の完全なエディタは目指さない。新規依存の追加は原則しない方針（ファジーマッチ・git/gh 連携・編集バッファ・クリップボードは依存を足さず自前実装 / CLI 呼び出しで済ませている）。

このファイルは**複数のコンポーネントに跨る規約**だけを載せる。機能ごとの設計判断は `docs/design/`（末尾の索引）にあり、その機能を触る時にそちらを読む。

## コマンド

```sh
cargo build            # 警告ゼロを維持する
cargo run -- <dir>     # 起動（dir 省略時はカレント）。日常使いは --release（debug は syntect 初期化で起動に 1-2 秒）
cargo clippy
cargo fmt
```

見た目の確認は TUI を起動せず**静的プレビュー**で回せる（Compose の `@Preview` / SwiftUI Preview 相当。詳細は docs/design/preview.md）:

```sh
cargo preview                       # シーン一覧 (= cargo run --features preview -- --preview)
cargo preview git log               # 複数シーンを縦に並べて描き出す
cargo preview all --size 140x40
scripts/preview-watch.sh git        # 保存のたびに再ビルド + 再描画
cargo preview --update-snapshots    # docs/preview/*.svg を焼き直す (UI を変えたらコミットする)
cargo preview view --update-snapshots  # 1 シーンだけ焼き直す
```

速度は `cargo perf`（同じ dev 専用 feature の別入口）で測る。TUI を起動せず、1 打鍵ぶんの「キー入力 → 再描画」の所要時間を TSV で出す（docs/design/preview.md）:

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

- `app/` — 合成ルート。全ての状態を所有し、レーン/タブ遷移とキールーティングの優先順位を持つ。mod.rs(App 状態・on_tick・レーン/ワークスペース遷移・rescan/notice), keys.rs(キールーティングの優先順位とレーン/オーバーレイのキー処理), commit.rs(Mode::Commit の開閉・編集・実行), git_ops.rs(stage/discard/stash・fetch/pull/push の実行と後始末), file_ops.rs(ツリーのファイル操作: 新規作成・ディレクトリ作成・リネーム・削除・パスコピー), branch_ops.rs(Mode::Branch のキー処理と切替/作成), github_keys.rs(Issues/PullRequests タブのキー処理と gh ジョブ起動), mouse.rs, mode.rs(Focus/Lane/Mode/Workspace/InputKind/ConfirmAction)
  - keys.rs は**「どのキーを誰に渡すか」だけ**を持ち、操作の中身は上記 4 ファイルへ置く。keys.rs が肥大化して優先順位が読めなくなるのを避けるための分割なので、新しい操作を足す時もこの境界を守る（キーの追加は keys.rs、実行の中身は用途別ファイル）。モジュールを跨いで呼ぶメソッドだけ `pub(super)` にする
  - **キー処理をコンポーネント側へ移さないのはなぜか**: ハンドラはほぼ全て「複数のコンポーネントを跨いで App を書き換える」（GIT のキーが `App::rescan` を呼ぶ、issues のキーが `job::spawn` する等）。component 側へ持っていくと component → app の逆向き依存が生まれる。コンポーネント内で閉じる操作は既にその状態型のメソッド（`GitState::next_hunk` / `PrsState::set_open` 等）になっており、keys.rs はそれを呼ぶルータに徹している
  - 書き込み系操作の後の即時再取得は `App::rescan_now`（rescan + デバウンスのタイマー/保留フラグのリセット）に集約する。呼び出し側で 4 行を複製しない
- `component/` — 状態 + その状態だけを受け取る View。各フォルダは `mod.rs`（状態）と `view.rs`（描画）を持つ
  - `tree/` — mod.rs(選択・展開操作), node.rs, scan.rs(1 階層走査・遅延ロード・rescan ヘルパー), view.rs
  - `viewer/` — mod.rs(open/reload/履歴・cache), viewport.rs(Viewport: スクロール・折返し状態), highlight.rs(Highlighter: syntect 一式とテーマ + 行単位で再開できる Session/LineState), render.rs(HighlightCache: 可視範囲の前後に余白を持つ帯として組み立てる遅延ハイライト), content.rs(読込・Content/TextDoc/Open), search.rs, selection.rs(Selection: 範囲選択とコピー用のテキスト取り出し), rowcursor.rs(行カーソルの追従計算。VIEW/GIT/コミット diff/PR が共有), view.rs
  - `editor/` — mod.rs(EditState: カーソル・キー処理・追従), buffer.rs(EditBuffer: 生テキスト・undo/redo), diff.rs(prefix/suffix トリム + LCS と、その共通範囲を打鍵を跨いで持ち越す CommonTrim。行単位のライブ diff と gitlane の word-level diff が LCS を共有する `pub(crate)`), view.rs
  - `gitlane/` — GIT レーンの diff 表示状態。mod.rs が GitState (今どの diff をどう見ているか・行カーソル・行単位選択) と定数/Kind/各 *Diff 構造体、render.rs が inline の行組み立て (render_inline / コミット一覧パネル・PR タブと共有する render_commit)、side.rs が side-by-side (#30)、word.rs が word-level 差分の範囲計算 (#29)、patch.rs が行単位 stage のパッチ組み立て、view.rs が描画
  - `log/` — コミット一覧パネル (`L`) の一覧・ページング・選択 diff の状態 + view.rs(コミット一覧 + diff)
  - `issues/` — issues タブの一覧フィルタ・詳細キャッシュ・ジョブ管理 + view.rs(一覧 + 詳細)
  - `prs/` — pull requests タブの一覧フィルタ・説明/diff/CI 3 種のキャッシュ・ジョブ管理 + view.rs(一覧 + 説明/diff/CI)
  - `remotelist/` — issues/PR が共有する一覧フィルタ (`filter_rows`) と詳細の非同期キャッシュ (`DetailSlot`) + view.rs(一覧・プレーンテキスト詳細の描画部品)
  - `branch/` — BranchState(ブランチ一覧オーバーレイの絞り込み・選択状態) + view.rs
  - `finder/` — mod.rs(ファジーマッチ自前実装), index.rs(FileIndex: Finder 候補の背景全走査), view.rs
- `shell/` — 画面全体の骨格と、App 全体を横断して見せる画面。mod.rs(draw・レイアウト・各 View への値の取り出し), status_bar.rs, tab_bar.rs(Workspace タブバー), help.rs, settings.rs, confirm.rs(確認オーバーレイ), commit.rs(コミットメッセージ入力オーバーレイ)。ここに置くか component に置くかの境界は「専用の状態型を持つか」（「描画の依存範囲」節）
- `widget/` — 複数のコンポーネントが使う描画部品。text_pane.rs(閲覧・編集・diff 共通の描画コア + 行カーソル/行選択の帯 + `line_body`), diff_boundary.rs(sticky header の帯)。**帯の幅は必ずセル幅で測る**（`Line::width` / `text::cells`）— char 数で数えると全角のパスやコードで帯がペイン幅を超えて罫線を押し出し、ZWJ 絵文字では逆に右端が空く（「桁位置の整合インバリアント」の帯版）, icons.rs, mod.rs(pane_block / centered_rect)。**どの状態を描くかは持たせない**（渡された Line 列をどう見せるかだけ）
- `preview/` — mod.rs(`--preview` の入口・TestBackend への 1 フレーム描画), scene.rs(シーン定義＝プレビューしたい状態の一覧), keys.rs(シーンを組み立てるキー列 DSL), render.rs(Buffer → ANSI 文字列。手元で見る stdout 用), svg.rs(Buffer → SVG。スナップショット兼 README の画面写真), snapshot.rs(マスクとファイル書き出し), fixture.rs(固定サンプルリポジトリ)。開発用の入口で、アプリ本体からは呼ばれない（docs/design/preview.md）
- インフラ（どのコンポーネントにも属さない）: `text.rs`(タブ幅・gutter 幅・桁変換の唯一の定義) / `lang/`(UI 文言の言語とキー別の翻訳表。docs/design/ui-text.md) / `clipboard.rs`(クリップボードへの書き出し。外部コマンド → OSC 52 のフォールバックと自前 base64) / `git/`(git CLI ラッパー。mod.rs が実行レイヤ (run_git / run_git_write と出力整形) と全再エクスポート、status.rs(porcelain パース)・diff.rs(changed_lines/baseline_lines/file_diff/diff_all/truncate_diff)・log.rs・write.rs(stage/unstage/discard/commit)・component/branch/mod.rs(branches/branch_status/switch 系)・remote.rs(fetch/pull/push) にコマンドを分ける。呼び出し側から見えるパスは分割前と同じ `git::foo`) / `github.rs`(GitHub モードが使えるか 1 箇所で判定する check_available に加え、gh CLI ラッパー: issues/PR 一覧・詳細取得の `list_issues`/`issue_detail`/`open_issue_web`/`list_prs`/`pr_detail`/`pr_diff`/`pr_checks`/`open_pr_web`) / `job.rs`(非同期ジョブの基盤。thread::spawn + mpsc::channel の薄いラッパー) / `watch.rs`(notify) / `config.rs`
- **可視性**: component/widget は別のモジュールツリーから呼ばれるので、跨いで使うものは `pub(crate)` になる（レイヤ別構成なら `component/*/view.rs` 内で `pub(super)` に閉じられていた分の代償）。フォルダ内に閉じるものは `pub(super)` のままにする

### Workspace（タブ）・レーン（Lane）・オーバーレイ（Mode）の3軸
キーマップ飽和を避けるため、状態を3軸に分けている。**新しい機能を足す時はどの軸かをまず決める**。
- `Workspace`（app/mode.rs）= トップレベルのタブ。`Viewer` / `Issues` / `PullRequests` の3つで、GitHub モード（既定 off）有効時だけ **Ctrl+t で循環**（`App::cycle_workspace`）・Alt+1..3 で直接指定・タブクリックで切替。`Workspace::Viewer` が既存アプリ全体（Lane 3 種 + ツリー + オーバーレイ）にあたり、Issues/PullRequests は「ローカルのファイル」という文脈を共有しないリモートのデータなので Lane には混ぜない（Shift+Tab で編集中から PR 一覧に飛ぶとレーンの意味が壊れるため）。GitHub モードが無効/使えない間は Workspace は Viewer 固定で、タブバーの1行も確保しない（`shell::draw` が `App::workspace_available` 1 箇所で判定）
- `Lane`（app/mode.rs）= Viewer タブの中の持続する作業レーン。`View` / `Edit(EditState)` / `Git(GitState)` の3つで、**Shift+Tab で循環**（`App::cycle_lane`）。Edit・Git は自分の状態を所有し「そのレーンにいるのに状態が無い」を型で排除する。コミット履歴はかつて 4 つ目の `Log(LogState)` だったが、「ファイルを読みながら履歴も追う」が実際の使い方で、画面を丸ごと差し替えるレーンにするのは強すぎたため VIEW 内のパネル（下記）へ畳んだ
- `Mode` = レーンの上に重なる一時オーバーレイ（Input/Finder/Help/Settings）。閉じると `Mode::Normal` に戻るが**レーンは変わらない**（GIT でヘルプを開いて閉じても GIT に戻る）。この分離のために `Mode::Edit` を `Lane::Edit` へ移した経緯がある。Workspace を跨いでも同様にモードは独立している
- 入れないレーンは循環時にスキップする（非テキスト → EDIT、変更が無い → GIT）。判定は `enter_edit` / `enter_git` が false を返す形に閉じ込め、呼び出し側で条件を二重に書かない。GIT (`git_available`: 変更が1件以上) とコミット一覧パネル (`log_available`: git repo でありさえすればよい) は判定基準が違う点に注意（一覧はコミット 0 件の repo でも「no commits」を見せるだけで良いため）
- **Shift+Tab は Edit レーンより前に処理する**（keys.rs）。印字キーではないので「編集中は印字キーを全て文字入力にする」ポリシーとは衝突しない。ただし未保存バッファがある間はレーンを変えず notice を出す。Issues/PR タブに Lane の概念は無いので、そこに居る間 `cycle_lane` 自体が no-op になる（ステータスバーのレーンセグメントも合わせて暗くする）
- `Focus`（Tree/Log/Viewer）はレーンと直交する。GIT でも Tab で左右を行き来する。`Log` はコミット一覧パネル（docs/design/git.md）が実際にペインとして増えるぶんで、issues/PR や GIT のように「左ペイン/右ペイン」の意味を再利用できないため増やした唯一の variant。パネルを出していない間の Tab は Tree ⇄ Viewer のままで、`App::cycle_focus` が `log_panel_visible()` を見て 3 ペイン循環に切り替える
- 右ペインの中身はレーンと「最後に開いたもの」で決まる（VIEW: ファイル or 選択コミットの diff / EDIT: 編集バッファ / GIT: diff）。`shell::draw` の振り分けがその唯一の場所で、コミット diff を出しているかどうかの判定は `App::showing_commit_diff` 1 箇所（描画・キールーティング・マウス・ステータスバーが全てここを見るので「見た目は diff なのにキーはファイル側へ効く」が起きない）
- **未保存の編集バッファがあっても Workspace の切替は拒否しない**（`Lane::Edit` の状態はタブを跨いでも保持され、Viewer タブへ戻れば復元される）。Shift+Tab のレーン循環がバッファ dirty 中に拒否するのとは対照的で、その代わりタブ側に未保存マーク（`viewer ●`）を出す
- GitHub モードの有効化は起動オプション `--github` / 設定オーバーレイのトグル / config ファイル `github = true` の3経路が同じ `Config.github` に集約される。`--github` はその起動限りの上乗せで config には書かない（`App::github_enabled` と永続化用の `github_persisted` を分けて持つのはこのため）。`gh` の有無・認証・GitHub リモートかどうかの判定（`github::check_available`）は起動時（または初回有効化時）に1度だけ行い、描画のたびには叩かない
- `Mode::Help { scroll }` は**読み位置だけを持つ**。全レーン分のキー一覧は 190 行近くあり、端末の高さ（オーバーレイは 80%）に収まらない。以前は `Paragraph` に全行を渡すだけで、**溢れたぶん（Tree 以降のほぼ全セクション）が黙って切れていた**ため「Git のコマンドがヘルプに無い」ように見えていた。他のペインと同じく**自前でスライス**して描き（`Paragraph::scroll` は使わない）、実測（表示行数・総行数）を `App::help_view` へ書き戻して `on_help_key` がクランプとページ送り量に使う（`viewport.height` と同じ 描画→app のパターン）。枠のタイトルに `1-24/188` を出すのは、切れているのか終わりなのかを区別できるようにするため。さらに**今開いている画面の節を先頭へ持ち上げる**（`shell/help.rs` の `current_screen`（Workspace/Lane/Focus + `showing_commit_diff` を節の粒度まで潰す）→ `hoisted`（持ち上げる `SectionId` の列））— スクロールできるようにしただけでは「GIT を見ているのに Git の節は 5 画面下」が残り、「今押せるキー」に辿り着けないため。並び替えるのは順序だけで、節の中身も**全節を載せること**も変えない（探しに行けば必ずそこに在る、を壊さない）。持ち上げた節の見出しには `← 今の画面` を付け、なぜ並びが変わったのかを画面上で説明する。ヘルプ自身の操作（`Help (?)`）は持ち上げた節の直後に固定する — 今の画面の節が 1 画面を超えて押し出されても、同じ案内はステータスバーに常時出ている。専用の状態型を作るほどではないので `Mode::Commit` と同じくフィールドを直接持つ（＝ shell 側の画面）。並び替えのために `draw_help` だけは `&App` を受け取るが、これは status_bar/settings と同じ「シェル側の画面」の扱い
- `Mode::Confirm { prompt, action }`（破壊的・書き込み系操作の確認）も Lane と直交する。これまでの Mode（Input/Finder/Help/Settings）は編集中は開けない制約があったが、Confirm だけは EDIT レーン中でも出す必要があるため、キールーティング上は Shift+Tab と同じ位置（`Lane::Edit` の文字入力ディスパッチより前）に置く。`action` はクロージャではなく enum（`ConfirmAction`）にする — クロージャだと App を借りたまま呼べず、確認後に App のメソッドを呼ぶ形にできないため。書き込み系の子 issue が増えるたびに variant を足していく想定（最初の実装が `ConfirmAction::Amend`）。確認中は y/Enter 以外の全キーで中止し、他のキーがレーンへ漏れないことをキールーティングの順序で保証する（型ではなく手続きで守っている点は他の Mode と同じ）
- `Mode::Commit { buffer, cursor, amend, error }`（コミットメッセージ入力、`c`/`C`）も Lane と直交する独立オーバーレイ。`Mode::Input` は 1 行入力専用で複数行のコミットメッセージを表現できないため分けた。`buffer` は改行込みの生テキスト、`cursor` はバイトではなく **char インデックス**（日本語等の複数バイト文字でカーソル位置がずれないため）。`c`/`C` はキールーティング上グローバルキー（q/s 等）と同じ位置に置き、GIT レーンにいることを要求しない（変更を見て回ってからそのままコミットしたい時に Shift+Tab を挟ませたくないため）。可否は `App::has_staged_changes` 等で都度判定する
- `Mode::Branch(BranchState)`（ブランチ一覧、`b`）も Lane と直交する独立オーバーレイ。`c`/`C` と同じ位置（グローバルキー相当）に `b` を置き、GIT レーンにいることを要求しない。`BranchState`（component/branch/mod.rs）は Finder と同じ「絞り込み候補 + 選択位置」のパターンだが、current マーク・local/remote 区別・upstream・相対日時・件名という Finder の `candidate: String` 1本では表現できない付随情報を持つため専用の型にし、マッチングだけ component/finder/mod.rs の `fuzzy_match`（`pub(crate)` に公開）を再利用して新しいマッチャを書かない。可否 (`App::branch_available`) はコミット一覧と同じ基準（git repo でありさえすればよい）

### 行カーソル（どのペインでも「今どの行が対象か」を 1 行に確定させる）
以前はどのペインにも行カーソルが無く、「今どこを見ているか」は**上端に見えている行**という暗黙の基準に乗っていた。これが実際にバグを生んだ（GIT レーンの `Space` が `viewport.scroll` から hunk を引いていたため、画面に収まる diff ではスクロールが動かず常に hunk 1 を stage していた）ので、テキストを出す全てのペインが明示的なカーソルを持つ。
- **持ち主はコンポーネント、計算だけ共有**（`component/viewer/rowcursor.rs`）。「何行あるか」「1 論理行が何視覚行を占めるか」の求め方がペインごとに違う（diff ペインは `Line` の span を連結、VIEW は `TextDoc::plain`）ため、状態ではなく追従の計算（`scroll_for` / `clamp_cursor` / `line_at_row`）だけを純関数で置く。**純関数なのは借用のため** — 呼び出し側は `&mut self.viewport` と `self.lines()`（self 全体の不変借用）を同時には取れないので、「新しい値を計算して返す → 代入する」の 2 段にする必要がある
- **キーの意味**: `j`/`k`・`Ctrl+d`/`u`・`gg`/`G` はスクロールではなく**カーソル**を動かし、画面はそれに追従する。逆にホイールのスクロールはカーソルを画面内へ引き戻す（`clamp_cursor`）— 対象が画面外に居るまま実行キーを押せる状態を作らないため。`]`/`[`・`}`/`{`・検索ジャンプ・`:N` もカーソルを連れて動き、クリックでも動かせる
- **折返し中の下端は視覚行数に依存する**ので、`scroll_for` はカーソルから上へ `text::wrap_rows` で遡って最小の scroll を出す（O(画面行数)）。非 wrap と side-by-side（描画側が事前に行数を揃える）は視覚行 = 論理行なので単純な引き算で済む
- **描画は `TextPane::focus_row` / `selected_rows` の帯**（`widget/text_pane.rs`）。char 単位の `cursor`（REVERSED、EDIT 専用）とは別物で、行全体の**背景だけ**を差し替える。既に背景を持つ span（word-level 差分・検索マッチ）は残す — 帯で塗り潰すと「どの文字が変わったか」が読めなくなるため。行末からペイン幅までの穴埋めは `widen_row_bands`（`widen_boundary_bands` と同じ、描画側だけの後加工）
- **帯を出すのはフォーカスのあるペインだけ**（EDIT は常にモーダルなので常時）。ツリー操作中の右ペインに帯だけが残っていても、そのキーがそこへ効かない以上ただの雑音になる
- **プロース（issues の詳細・PR の説明/CI）には持たせない**。折返しの効いた散文では「1 論理行」が段落まるごとになり、行を単位にした帯もカーソル移動も単位が合わない。`PrsState::move_cursor` は diff 以外の表示では素直な `scroll_by` へ落ちる
- **状態を作る側は右ペインの実測サイズを引き継ぐ**（`GitState::new` / `LogState::new` の引数、`PrsState::seed_viewport_size` / `set_open`）。diff ペインは VIEW/EDIT とまったく同じ Rect を使うので、レーン・表示を切り替えた直後（まだその Viewport への書き戻しが無い時点）の 1 打鍵でカーソル追従が暴れないようにするため

### 閲覧と編集の関係（後付けにしない）
- `Viewport`（scroll/hscroll/wrap/実測サイズ）は閲覧・編集で**同じ実体を共有**する。モード遷移で位置が飛ばない根拠はここ。「wrap 中は hscroll = 0」のインバリアントは Viewport のメソッドと EditState::ensure_visible が守る（モード出口での手当てはしない）
- `Highlighter` は syntect のシンタックス定義とテーマの置き場で、ハイライト結果も行の状態も持たない。EditState は Viewer 全体ではなく **Viewport だけを借りる**（保存だけは cache 即時更新のため `Viewer::reload` を呼ぶ）。編集操作の経路（`handle_key`/`paste`）は Highlighter に触れず、借りるのは描画時（`component/editor/view.rs`）だけ。editor に新しい操作を足す時もこの依存範囲を広げない
- **ハイライトは可視範囲の前後に余白を持つ「帯」として描画時に組み立てる**（`component/viewer/render.rs` の `HighlightCache`）。閲覧・編集がそれぞれ 1 つ持ち、`TextPane` へは文書全体ではなく可視ウィンドウ（`widget/text_pane.rs` の `LineWindow`）を渡す。syntect は前の行の状態に依存する逐次処理なので、任意の行から再開できるよう `CHECKPOINT_STRIDE`(32) 行ごとにパーサ状態（`LineState` = ParseState + HighlightState）を保存し、届かない場所へ飛ぶ時だけそこから助走する。**帯 (`Band`) は行そのものに加えて「その行を解析する直前の状態」も行ごとに持つ**。これで「開くコストがファイルの大きさに比例する」「編集の度に全再ハイライトする」「1 打鍵ごとに画面 1 枚を作り直す」が同じ 1 つの仕組みで消える（checkpoint は助走で歩く過程で埋まるので、末尾へジャンプした時だけ一度全体を歩く）:
  - `j`/`k` の 1 行スクロールは帯の端に 1 行足すだけで済む（画面 1 枚を作り直さない）。下へは帯の末尾の状態から続けられるので助走ゼロ、上へは checkpoint からの助走が要るが、**帯の下端を checkpoint 境界に揃える**ことで助走を払うのは STRIDE 行に 1 度だけになる（しかも助走で歩いた行はそのまま帯に残すので捨てる仕事にならない）
  - 1 文字のタイピングは変更行を作り直し、**パーサ状態が元へ戻った時点で打ち切る**（`Band::repaint`）。文字列やコメントを開閉しない限り実際に作り直すのはその 1 行だけになる
  - 帯は可視範囲の前後 `BAND_SLACK` 行に `trim` で収める（スクロールし続けても際限なく伸びない）。落とすのは要求範囲から遠い側だけなので、帯はスクロールの向きに付いてくる
- 無効化は 3 通りだけ: `reset`（ファイルを開く/読み直す）・`invalidate_all`（テーマ切替）・`invalidate_from(touched)`（編集）。`invalidate_from` が「touched.from より手前から始まる checkpoint は行番号がずれないので残せる」ことを使って、変更行より前を再ハイライトしないことを担保する。`touched` はカーソル位置ではなく `EditBuffer::take_touched`（挿入・削除プリミティブが記録したもの）から取る — undo/redo は任意の位置に飛ぶため。**起点だけでなく終点と「行が増減したか」(`Touched { from, to, shifted }`) を持つ**のは、上記の打ち切りが成り立つ条件がその 2 つで決まるから: 中身が変わったのは `[from, to]` だけなので以降の行はパーサ状態さえ戻れば使い回せるが、行が増減すると帯の同じ位置が別の行を指すのでその判断自体が成り立たなくなる
- 描画は `widget/text_pane.rs` の `TextPane` に一本化（閲覧 = search あり cursor なし / 編集 = cursor あり search なし）。行加工順は `mark_changed_gutter → highlight_matches → highlight_selection → tint_row → (hscroll | セル単位 wrap) → cursor overlay` 固定。各段は `Line` ではなく **`Vec<Span>` を受け取って返す**形で、加工しない span は中身を複製せず借りたまま (`borrowed`) / 所有したまま (move) 引き継ぐ — 段ごとに `Line` を deep clone すると、何も加工しない既定の状態でも 1 フレームの確保が「可視行数 × span 数」に比例して積み上がる
- wrap は閲覧・編集とも **セル単位（端末の表示幅単位）の自前分割**（`Paragraph::wrap` は単語境界 wrap で折返し位置が外から計算できないため全面的に不使用）。**折返し規則の唯一の定義は `text::WrapCursor`** で、描画（text_pane の `wrap_line` / gitlane の `wrap_split`）・視覚行数（`text::wrap_rows`）・カーソル追従（`text::wrap_position`）・クリック座標（`text::wrap_col_at`）の 4 者がこれを共有する。ズレると即カーソル位置バグになる
  - **char 数で詰めてはいけない**（実際に踏んだバグ）。ratatui の描画（`LineTruncator`）は grapheme の display width で桁を送り、幅を超えた時点でその行を打ち切るので、全角 1 文字を 1 桁と数えて詰めると視覚行が幅を超え、**はみ出した文字は次の視覚行にも現れないまま消える**（日本語のコメント行で「折返しの継ぎ目から数文字抜ける」形で表面化した）。全角が境界を跨げないぶん行末にセルが余ることもあるため、視覚行数も `display_len / width` の割り算では出せない（`wrap_rows` が本文そのものを受け取るのはこのため）
  - **走査は char ではなく grapheme 単位**。ZWJ 絵文字（`👩\u{200d}💻`）のように「char ごとの幅の合計（4）と実際の描画幅（2）が食い違う」列があり、char で数えると幅を過大に見積もって列の途中で割れる（絵文字が 2 つの視覚行に分かれる／クリック座標が数セルずれる）。セル幅は `text::cells`、grapheme 分割は ratatui の `Span::styled_graphemes` — unicode-width も unicode-segmentation も直接足さず ratatui を通すのは、描画側とまったく同じ計算・同じ単位であることを保証するため（新規依存も増やさない）

### キールーティングの優先順位（app/keys.rs on_key）
Ctrl+c → Mode::Confirm → Mode::Help → Mode::Settings → Mode::Finder → Mode::Input(Search/Goto/Filter) → Mode::Commit → Mode::Branch → **Ctrl+t/Alt+1..3(Workspace 切替)** → **Shift+Tab(レーン循環)** → **Workspace ≠ Viewer なら以降をスキップし on_issues_key/on_pr_key へ**（Issues は `on_issues_key`、PullRequests は `on_pr_key` がそれぞれ focus 別に一覧/詳細へ振り分ける。どちらも「フォーカスに依らない操作 (o/r/t/フィルタ開始) を先に拾い、残りは focus で振り分け」という同じ形） → Lane::Edit → Ctrl+p → q/?/a/i/s/c/C/b/f/p/P/**L(コミット一覧パネル、VIEW 限定)**/Tab → **Z(stash pop、レーン不問)** → **X/z(discard・stash push、GIT レーン限定)** → focus 別ディスパッチ。f/p/P (#27 リモート操作) は c/C/b と同じ位置・同じ理由 (レーンを問わず開ける) でここに置く。新しいモード・キーを足す時はこの順序に組み込む。`L` だけは他のグローバルキーと違い `Lane::View` を条件に付ける（右ペインにコミット diff を出せるのが VIEW だけなので、GIT/EDIT で押しても意味を持てない）。VIEW の範囲選択キー（v/y/Y/Esc）は `on_viewer_key` の中（focus 別ディスパッチの先）に置く — レーンを跨がない閲覧専用の操作なので、c/C/b のようにグローバルへ持ち上げない。Edit はグローバルキー（q/s/Tab/Ctrl+p）より前に置くことで印字キーを全て文字入力にしている（Ctrl+c と Shift+Tab だけが上に残る）。Shift+Tab をオーバーレイ判定より後ろに置いているのは、入力中にレーンが切り替わって文脈が壊れないようにするため。Ctrl+t/Alt+N も印字キーではないので同じ位置（オーバーレイ判定の後・Lane::Edit の前）に置ける。`workspace_available` が false の間はこれらのキーが素通りするだけなので、GitHub モード無効時の挙動は 1 バイトも変わらない。`pending_g`（gg 待ち）は Tree/Viewer で共用され、Tab・マウスでリセットされる。Z (stash pop) だけレーンを問わず呼べる理由は docs/design/git.md「破棄 (discard) と stash」を参照。
GIT レーン右ペインの `Space`（hunk 単位ステージ）と `Enter`（行単位ステージ）は `on_git_key` の中で、`A`/`t` と同じく `Lane::Git` の可変借用より前で拾う（git の実行と `rescan_now` に `&mut self` が要るため）。ツリー側（Focus::Tree）の `Space` はファイル単位のトグル・`Enter` は diff を開く操作で、粒度と意味がフォーカスで変わる。
ツリーのキー処理（`on_tree_key`）は VIEW/GIT で共通で、**「開く」対象のパスを返すだけ**にしてある。viewer に開くか diff に開くかの振り分けは `App::open_selected` 1 箇所に閉じている（ツリー操作をレーンごとに複製しない）。コミット一覧は別ペインなので `Focus::Log` の分岐が `on_log_list_key` を直接呼び、右ペイン側は `showing_commit_diff()` の時だけ `on_log_diff_key` を割り込ませる。`L`（パネルのトグル）は c/C/b と同じグローバルキーの位置に置くが、そこだけ `Lane::View` を要求する（右ペインでコミット diff を出す場所が VIEW にしか無いため）。

### 桁位置の整合インバリアント（複数ファイルに跨る前提）
- 各行 `Line` の **span[0] は行番号 gutter**。検索ハイライト・水平スクロールは span[1..] を char 単位で走査する
- **閲覧の `cache: HashMap<PathBuf, Cached>` は上限付き・stat 照合付き**（`Viewer::cached_or_load`）。以前は開いたファイルを無制限に持ち続け、さらに**開いていない間に外から書き換えられたファイルは古い内容のまま開き直されていた**（watcher の reload は current にしか届かず、cache は捨てられなかった）。開く時に (mtime, size) を照合して違えば読み直し、総量が `MAX_CACHE_BYTES`（64MB）を超えたら使っていない順に捨てる（今開いているものは残す）。開いていないファイルの変更通知は `Viewer::forget` で cache から落とすだけ（読み直さない）。EDIT の undo も同じ理由で `MAX_UNDO`（500 単位）で頭打ち
- `Content::Text(TextDoc)` の `plain` は normalize 済み（改行除去・タブ→スペース4）で、**char インデックスが「描画桁」= 検索マッチ・選択・カーソルが使う座標と 1:1 対応**する（全角文字は 1 char = 2 セルなので、端末のセル桁と一致するのは半角だけ。セル桁が要るのは折返し計算だけで、そちらは `text::WrapCursor` に閉じている）。検索マッチの (line, start_col, end_col) はこの前提で 描画側の bg 重ねに直結する。`TextDoc` はハイライト済みの `Line` を持たず（`raw` = syntect へ渡す生の行 と `plain` の 2 本だけ）、色は `HighlightCache` が可視範囲ぶんだけ後から付ける。だからテーマを切り替えても `HashMap<PathBuf, Rc<Content>>` の cache は捨てなくてよい
- タブ幅・gutter 幅・表示桁⇔char 座標の換算は **`text.rs` が唯一の定義**。閲覧（component/viewer/content.rs の normalize）と編集（カーソル・クリック座標）が別々に持つとここが最初に壊れる
- 大文字小文字の畳み込みは ASCII 限定（`to_ascii_lowercase`）。Unicode の完全 case folding は char 数が変わり桁対応が壊れるため意図的に使っていない（component/viewer/search.rs と component/finder/mod.rs の両方）
- text_pane の行加工順は `mark_changed_gutter → highlight_matches → highlight_selection → tint_row → hscroll_spans` 固定。hscroll を先にすると検索マッチ・選択範囲の絶対桁がズレる
- gutter の変更行マーク `▎` は「gutter 末尾の空白 1 文字を置き換える」方式で char 数を維持している

### 描画の依存範囲（View は自分の状態しか受け取らない）
各ペイン・オーバーレイの `draw_*` は **そのコンポーネントの状態 + 描画に要るスカラ（`focused` / `background` 等）だけ**を引数に取り、`&App` は受け取らない。理由は 2 つある。
- **借用**: `GitState` も `EditState` も `App` の中にあるので、`&App` と `&mut app.lane` は同時に取れない。呼び出し側（`shell::draw`）が先に必要な値を取り出してから子へ渡す形にしないとそもそもコンパイルが通らない（`component/gitlane/view.rs` の先頭コメントがこの経緯）
- **範囲を型で縛る**: View が触れる状態が引数の型で決まるので、「このペインを直すのにどこを見ればいいか」がシグネチャだけで分かる。App 全体を渡すと、あとから無関係なフィールドを読み始めても誰も気づけない

例外は **シェル側の画面**（`shell/` 配下: status_bar / settings / confirm / commit / tab_bar）で、これらは本質的に App 全体の状態・設定を横断して見せるものなので `&App` のままにしてある。「専用の状態型を持つか」が境界の目安で、`Mode::Finder(Finder)` / `Mode::Branch(BranchState)` のように状態型がある側はコンポーネント扱い、`Mode::Commit { .. }` のように `Mode` の中に直接フィールドを持つ側はシェル扱いになる。この境界はそのまま**フォルダの置き場所**（`component/` か `shell/` か）と一致する。

### 描画は自前スライス
`Paragraph::scroll` は u16 上限で使わない。`lines[scroll..scroll+height]` を毎フレームスライスして描画する（text_pane）。ui は `viewport.height` / `viewport.width` / `tree_area` / `viewer_area` / `splitter_area` を毎フレーム App/Viewport に書き戻し、キー・マウス処理側がそれを読む（描画→app の逆流はこのパターンに統一）。

### 機能別の設計ノート（docs/design/）
上の節は複数のコンポーネントに跨る規約だけを載せている。個々の機能の「なぜこの形か」は `docs/design/` に機能ごとに置いてあり、**その機能を触る時に読む**。新しい機能の設計判断もここに書き足す（CLAUDE.md に戻さない）。

| ファイル | 内容 |
| --- | --- |
| [tree.md](docs/design/tree.md) | ツリー走査（1 階層ずつの遅延ロード・compact folders）・FS 監視と rescan の分類・Finder の候補・ツリーのファイル操作 (n/N/R/D/y)・ツリーペインの描画・ペイン幅のドラッグリサイズ |
| [git.md](docs/design/git.md) | git CLI ラッパー・GIT レーン（diff 表示・hunk/行単位 stage・word-level・side-by-side・diff 内検索・まとめ diff）・コミット・discard/stash・ブランチ一覧・コミット一覧パネル・非同期ジョブと fetch/pull/push |
| [github.md](docs/design/github.md) | GitHub モードのタブバー・issues タブ・pull requests タブ・両者が共有する一覧/キャッシュ基盤 (remotelist) |
| [grep.md](docs/design/grep.md) | ワークスペース横断検索 (`Ctrl+f`) の恒久的な要約（作業メモは [workspace-grep.md](docs/design/workspace-grep.md)） |
| [viewer-editor.md](docs/design/viewer-editor.md) | ビューアの範囲選択とコピー・インライン編集（EditBuffer・undo・ライブ diff・単語移動） |
| [ui-text.md](docs/design/ui-text.md) | UI 言語（`lang/`、文言の足し方）・一時通知 |
| [preview.md](docs/design/preview.md) | UI プレビュー・スクリーンショットテスト（SVG スナップショット・CI コメント）・速度チェック (`cargo perf`) |

## スタイル

- コメントは Why のみ・日本語。What の説明やコード写経コメントは書かない
- 再描画のコストを画面の大きさより上に持ち上げない。文書全体に比例する処理（全行の再ハイライト・全行分の `Line` 組み立て）を描画パスやキー処理に足さないこと。テキストは `HighlightCache`（帯 + checkpoint）、ツリーは `component/tree/view.rs::visible_window` が既にこれを守っている。**「1 打鍵で実際に変わった行数」まで落とせるならそこまで落とす** — ハイライトの帯・EDIT のライブ diff (`CommonTrim`) はどちらも「触っていない行の結果は変わらない」を使って、画面の大きさぶんの仕事すら省いている
