# UI preview gallery

<!-- UI スクリーンショットテストの成果物そのもの。`cargo preview --update-snapshots` が焼き直すので手で貼り替えない -->

`cargo preview <scene>` (see the [top-level README](../../README.md#ui-previews)) が描き出す全シーンの一覧。
各画像は `docs/preview/<scene>.svg` として commit 済みで、CI が毎 PR で再描画して突き合わせる。

## VIEW レーン

| | |
| --- | --- |
| **tree** — ツリーだけ (ファイル未選択) | **view** — VIEW レーン: ファイルを開いた既定の画面 |
| <img width="500" src="tree.svg" /> | <img width="500" src="view.svg" /> |
| **wrap** — 折返し表示 (`w`) — 続き行の gutter pad | **view-binary** — 非テキストファイルのフォールバック |
| <img width="500" src="wrap.svg" /> | <img width="500" src="view-binary.svg" /> |
| **search** — 検索ハイライトと `n`/`N` の状態 | **narrow** — 狭い端末 (列が落ちる閾値の確認) |
| <img width="500" src="search.svg" /> | <img width="500" src="narrow.svg" /> |
| **select** — 行単位の範囲選択 (`v` → `j`) とコピーのヒント | **tree-ignored** — `i` で `.gitignore` 対象も表示 (暗色) |
| <img width="500" src="select.svg" /> | <img width="500" src="tree-ignored.svg" /> |

## EDIT レーン

**edit** — カーソル + 未保存バッファのライブ diff

<img width="500" src="edit.svg" />

## GIT レーン

| | |
| --- | --- |
| **git** — 単一ファイルの inline diff (word-level 強調) | **git-side** — side-by-side diff |
| <img width="500" src="git.svg" /> | <img width="500" src="git-side.svg" /> |
| **git-all** — 全ファイルまとめ diff (sticky header 付き) | **git-lines** — 行カーソルと `V` の行単位選択 (`Enter` で行だけ stage) |
| <img width="500" src="git-all.svg" /> | <img width="500" src="git-lines.svg" /> |
| **confirm** — `X` 破棄の確認オーバーレイ | |
| <img width="500" src="confirm.svg" /> | |

## LOG レーン

**log** — コミット一覧 + 選択コミットの diff

<img width="500" src="log.svg" />

## オーバーレイ

| | |
| --- | --- |
| **finder** — `Ctrl+p` ファジーファインダー | **help** — `?` ヘルプオーバーレイ |
| <img width="500" src="finder.svg" /> | <img width="500" src="help.svg" /> |
| **settings** — `s` 設定オーバーレイ | **commit** — `c` コミットメッセージ入力 (50/72 桁ルーラー付き) |
| <img width="500" src="settings.svg" /> | <img width="500" src="commit.svg" /> |
| **branch** — `b` ブランチ一覧オーバーレイ | |
| <img width="500" src="branch.svg" /> | |

## GitHub モード

| | |
| --- | --- |
| **issues** — issues タブ | **prs** — pull requests タブ |
| <img width="500" src="issues.svg" /> | <img width="500" src="prs.svg" /> |

---

シーンの定義は `src/preview/scene.rs`。焼き直しは `cargo preview --update-snapshots`（1 シーンだけなら
`cargo preview <scene> --update-snapshots`）。詳しい仕組みは [トップレベル README の「UI screenshot tests」節](../../README.md#ui-screenshot-tests)を参照。
