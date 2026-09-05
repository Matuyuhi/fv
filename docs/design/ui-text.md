# UI 言語と一時通知

> CLAUDE.md から切り出した設計ノート。

## UI 言語（lang/、設定画面の `language`）
- 文言は**キーで引く**: 固定文言は `lang::t(Msg::HelpQuit)`、埋め込みがあるものは `tr!(Msg::GitStagedLines, lines, verb = "stage")`（`名前 = 式`、または同名の変数があれば名前だけ。`format!` の暗黙キャプチャと同じ書き味）。翻訳表は**言語ごとに 1 ファイル**で、`src/lang/msg.rs` がキー一覧（`Msg` enum）、`src/lang/ja.rs` / `src/lang/en.rs` がそれぞれ `Msg` に対する match。**match を網羅させる**ことで「片方の言語だけ書き忘れた文言」がコンパイルエラーになる（以前の「呼び出し側に ja/en の対で書く」設計と同じ保証を、文言を 1 箇所へ集めた形で保つ）
- 文言を足す手順は 3 箇所: `msg.rs` に variant を足す → `ja.rs` と `en.rs` に文言を足す（片方を忘れると match の網羅性エラーで止まる）。variant 名は「置き場所の接頭辞（Help/Git/Status/Prs/…）+ 英語文言の要約」
- 埋め込みは `{name}` のプレースホルダ。表の文字列は `&'static str` なので `format!` には渡せず、`lang::fmt` が名前で置き換える。位置引数（`{}`）は持たない。両言語で同じ名前が揃っていることは `placeholders_match_between_languages` テストが `Msg::ALL` を舐めて担保する
- **値はプロセス全体の static**（`lang::set` / `lang::current`）。描画関数は「自分の状態しか受け取らない」設計で、gh/git の失敗メッセージは背景スレッドで組み立てられるため、引数で配って回ると全ての `draw_*` と notice の組み立てにシグネチャ変更が波及する。`App::new` が config の値で最初に `set` し、設定画面の切替（`App::cycle_lang`）は `set` + `persist_config` するだけで App にはフィールドを持たない
- config に無い時の既定は `Lang::detect`（`LC_ALL` > `LC_MESSAGES` > `LANG`、`ja` 始まりなら日本語、それ以外は英語）
- **プレビューは日本語固定**（`preview::preview_lang`）。`isolate_env` が `LC_ALL=C` にするので detect に任せると英語になり、既存のスナップショットが全部変わる。英語の絵は `FV_PREVIEW_LANG=en cargo preview <scene>` で見る
- 置き換える対象は**ユーザーに見える文言だけ**。テストの文字列・`assert!`/`expect` のメッセージ・コメントは日本語のまま

## 一時通知（App::notice と EditState.notice）
`App.notice: Option<(String, Instant, bool)>` は全レーン共通の一時通知で、GIT の書き込み結果などレーンを離れても見せたいメッセージに使う。`EditState.notice`（EDIT レーン専用・保存エラーや discard 確認に使用）とは役割を分けたまま両方残す — EditState 側は「Viewport だけを借りる」依存範囲の制約があり、App 全体の状態を持たせると設計が崩れるため統合しない。期限切れは `on_tick` でのみ判定し（`watcher` が無い環境でも on_tick 冒頭で判定するので消えなくなることはない）、再描画のたびにタイマーを触らない点は他のデバウンス系の方針と揃えている。ステータスバーでは `Mode::Confirm` の prompt → `App.notice` → レーン別ヒントの優先順で 1 行に出す

