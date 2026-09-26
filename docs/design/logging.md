# 診断ログ (`src/logger.rs`)

TUI 実行中の stdout/stderr は alternate screen + raw mode の画面そのものなので、`eprintln!` で
書いた診断は描画を崩した上で次のフレームに上書きされて残らない。失敗経路の「後から何が起きたか
を追える情報」はファイルへだけ書く。UI への表示 (notice・`Content::Error`・各タブのエラー行) は
従来どおり呼び出し側が持ち、**ログはその代わりにしない**（診断情報の追加に留める）。

## 出力先・レベル・初期化

- 出力先: `FV_LOG_FILE` > `$XDG_STATE_HOME/fv/fv.log`（相対パスの XDG_STATE_HOME は仕様どおり無視）>
  `~/.local/state/fv/fv.log`。config（`$XDG_CONFIG_HOME/fv/config`）と分けるのは、ログが
  「ユーザーの設定」ではなく XDG の state（再起動を跨いで残すが消してよいもの）だから
- レベル: `FV_LOG=off|error|warn|info|debug`、既定 `warn`。読み取り系 git の非ゼロ終了は
  正常系でも頻繁に起きる（upstream 無し・HEAD の無い repo・新規ファイルの HEAD 版）ので `debug`
- 初期化: `main` の先頭で `logger::init()`。`Config::load`（parse_command）と `App::new`
  （git / gh を叩く）がどちらも失敗経路を持つため、その前に置く。**ファイルは最初の 1 行を書く
  時まで開かない** — 何も起きなかった起動・`--help` でファイルを作らないため
- `init` が呼ばれていない間（ユニットテスト）は全ての呼び出しが no-op。テストがユーザーの
  ログを汚さない（`Logger` の単体テストは一時ディレクトリの別インスタンスを直接使う）
- 閾値の判定はメッセージの整形より先に行う。呼び出し側は `format_args!` を渡すので、既定の
  `warn` で `debug` を捨てる時に文字列を作らない

## 1 行の形

`2026-09-26T04:19:52Z 12345 WARN  git: git push failed (exit status: 128): fatal: ...`

- 時刻は UTC の ISO 8601。chrono 等を足さず日付計算は自前（`civil_from_days`）
- pid を毎行に入れるのは、複数の fv が同じファイルへ追記しうるため（`O_APPEND` で 1 行ずつ書く）
- 改行・ESC 等の制御文字は空白に潰し、500 char で切る。1 メッセージ = 1 行を保つのと、
  `cat` した端末でエスケープシーケンスが効かないようにするため
- 同じ (level, scope, message) が続く間は書かずに数え、別のメッセージが来た時・終了時に
  `(previous message repeated N times)` を 1 行出す。git が無い環境の rescan・監視エラーは
  同じ行を延々と繰り返すので、畳まないとファイルがそれだけで埋まる
- 1 MiB を超えたら `fv.log.1` へ 1 世代だけ退避する。新規作成は `0600`
- 複数の fv が同じファイルへ追記しうるので、開いたハンドルが覚えている大きさは信用せず、
  **1 行書く度にパスを stat し直す**（`still_current`）。実ファイルが上限を超えていれば退避して
  開き直し、別プロセスに退避された（パスの指す inode が変わった・消えた）なら新しいファイルを
  開き直す — そうしないと `.1` 側へ書き続ける。書くのは既定で警告以上なので stat のコストは
  問題にならない。2 プロセスがほぼ同時に退避すると片方の数行が `.1` ごと上書きされうるが、
  診断用なのでロックまではしない（inode で比べられない非 unix では大きさの判定だけ）

## 機密情報を書かない

第一の防御は**呼び出し側がそもそも渡さない**こと:

- git / gh は `logger::command_label` でサブコマンド名（git は 1 語、gh は 2 語）だけを残し、
  残りの引数（パス・ブランチ名・issue 番号）や stdin（コミットメッセージ・パッチ）は書かない
- 失敗理由は UI に出すのと同じ stderr の 1 行（`first_line` / `remote_error_line`）だが、
  **ログ用にだけ `logger::mask_command_output` を通す**。書かないと決めた引数も stderr に引用されて
  戻ってくる（不正なブランチ名・一致しない pathspec）ので、落とした引数の値（2 char 以上。
  `--source=X` は値だけ）と、引用符で囲まれた部分（`'…'` / `"…"`）を `[…]` に置き換える。英文中の
  アポストロフィ（`couldn't`）を引用と取り違えないよう、直前が英数字の引用符では開かない。UI に
  出す文言は加工しない
- stdin を渡すコマンド（`commit -F -` / `apply -`）は**終了状態だけ**を書き、stderr は書かない。
  commit の hook（pre-commit / commit-msg）はメッセージやファイルの中身を出しうるし、apply の
  失敗はパッチの行を引用するため。他のコマンドの hook（post-checkout 等）の出力は上の伏せ字を
  通った 1 行としてしか残らない。push/pull の失敗は `remote_error_line` が hook の出力より
  git 自身の `fatal:` / `error:` 行を優先して拾う
- ファイル読み込みの失敗はパスと `io::Error` の理由だけ

その上で `logger` 側が保険として伏せる: URL の userinfo（`scheme://userinfo@host`
の userinfo 部分。push/fetch の失敗は remote URL をそのまま stderr に出す）と GitHub トークンの
接頭辞（`ghp_` / `gho_` / `ghu_` / `ghs_` / `ghr_` / `github_pat_`）。

## I/O エラー時のフォールバック

書けない（ディレクトリを作れない・HOME 不明・ディスク満杯）と分かった時点で出力を止め、以後は
試さない（失敗する I/O を毎回繰り返さない）。TUI 中には**一切表に出さない** — stderr に出すと
画面が崩れ、notice に出すと本来のエラー表示を押しのけるため。`main` がコマンドの実行
（`run_command`）から戻った後に `logger::finish()` を 1 回だけ呼び、戻り値（理由と捨てた件数）を
stderr に 1 行だけ出す。TUI の経路では `run_app` が端末を戻し終えてから返るので画面は壊れず、
`--preview` / `--perf` / エラー終了でも同じ場所を通る（畳んだ「繰り返し N 回」の書き出しもここ）。
書けなくなった後のメッセージは畳まずに 1 件ずつ数える（畳んだ要約行は書けないので、要約 1 行
ぶんとしか数えられなくなる）。

## panic

panic hook（`main::install_panic_hook`）は端末を戻してから `logger::panic` を呼び、それから
既定の hook に渡す。ロックを持ったスレッド自身が panic していると `lock()` が返らないので、
ここだけ `try_lock` にして取れなければ諦める（panic メッセージ自体は既定の hook が stderr に出す）。

## eprintln! の扱い

`main.rs` の `#![warn(clippy::print_stderr)]`（CI は `-D warnings`）で `eprint!`/`eprintln!`
を禁止し、診断出力は `logger::{error, warn, info, debug}` へ寄せる。例外は端末を戻した後の
フォールバック 1 箇所だけ（`#[allow]` 付き）。`--help` / `--version` / `--preview` のように
TUI を起動しない経路の stdout 出力（`println!`）はこの規約の対象外。

## 使っている場所

| scope | 経路 | レベル |
| --- | --- | --- |
| `viewer` | ファイル読み込みの失敗（`component/viewer/content.rs::load`） | warn |
| `git` | 読み取り系の非ゼロ終了（`run_git`） | debug |
| `git` | 書き込み系・fetch/pull/push・stdin 付きコマンドの失敗 | warn |
| `git` / `github` | コマンドを起動できない（未インストール等） | error |
| `github` | gh の失敗（`run_gh` / `pr_checks`） | warn |
| `github` | GitHub モードが使えない理由（`check_available`） | info |
| `watch` | 監視スレッド・watcher の作成・`watch()` の失敗、監視中のエラー | warn |
| `config` | 設定の読み込み（未作成以外）・保存の失敗 | warn |
| `panic` | panic | error |

新しい失敗経路にログを足す時も、UI の表示は変えずに 1 行足すだけにする。
