# ツリー・FS 監視・レイアウト

> CLAUDE.md から切り出した設計ノート。横断的な規約（3 軸・行カーソル・桁のインバリアント等）は [CLAUDE.md](../../CLAUDE.md) 側にある。

## ペイン幅のドラッグリサイズ
左右の比率は `App::split_ratio`（config に永続化）。桁数でなく割合で持つのは端末リサイズで配分を保つため。割合→実桁の換算は `App::tree_width` 1 箇所だけで、ドラッグ時の clamp（`clamp_tree_width`: 最小幅を満たせない狭い端末では半分ずつ）も同じ関数を通す。ドラッグは `on_split_mouse` がレーン・オーバーレイ判定より前に処理して消費する（幅変更はレーンと直交する操作。編集中でも効かせる）。掴んだ桁のオフセットを `dragging_split` に持つので Down の瞬間に境界が飛ばない。config への書き込みはボタンを離した時だけ（ドラッグ中に毎フレーム書かない）。

## ツリー走査と FS 監視（起動をディレクトリの大きさから切り離す）
巨大なディレクトリで開くのに数秒かかっていたため、**起動時に触るのは root 直下 1 階層だけ**にしてある。「起動時にツリー全体を歩く」処理を足さないこと（`App::new` の所要時間がツリーの大きさに比例しない、が守るべき性質）。
- 走査は `scan::read_dir` の **1 階層ずつ**。`NodeKind::Dir` の `loaded` が未走査を表し、`scan::load` が展開の直前（`toggle_or_open` の開く側、`expand_all`）で子を読む。畳んだ子は捨てないので再展開はキャッシュヒットになる
- 1 階層でも `WalkBuilder` を通すのは、既定の `parents(true)` が**祖先の .gitignore を遡って読む**ため。サブディレクトリ起点でも root 側の `*.log` / `/anchored` / `build/` がそのまま効く（この前提が崩れるなら一括走査に戻すしかない）。`require_git(false)` で非 git ディレクトリでも .gitignore を尊重
- `rescan` は `scan::refresh` で**読み込み済みの階層だけ**を読み直し、展開状態と子を **name で**引き継ぐ（種別が変わったら引き継がない）。選択は **path で**保存・復元する（index_path は再走査で無効になる）。再走査コストも「今開いている範囲」に比例する
- **子がディレクトリ 1 つだけの階層は 1 行に畳む**（VSCode の compact folders。`com/example/app` のような中継ディレクトリを 1 段ずつ開かせない）。ノード構造は階層のまま変えず、`scan::flatten` が行を組む時に連鎖を `api/v1` の 1 行へ畳み、行の `index_path`/`path`/展開状態は**連鎖の末端ノード**のものにする（開閉・選択の path 復元・git status の照合が全てそこへ効く）。畳めるのは読み込み済みの範囲だけ（未走査の子は数えられない）なので、開く側は `scan::expand_single_child_chain` が連鎖を辿って読み込み、開いた瞬間に末端まで畳まれた形で見える
- `toggle_hidden` は show_hidden を反転して `rescan` するだけ（読み直しの経路を 2 つ持たない）。`toggle_ignored`（`i` / 設定画面の gitignored）も同じ形で、切替後の後始末（Finder 候補・FS 監視をツリーと同じ条件に揃え直す）は `App::after_scan_options_changed` 1 箇所に集約する
- **無視設定は `scan::ScanOptions`（show_hidden + show_ignored）が唯一の定義**で、`ScanOptions::walker` が `WalkBuilder` の組み立てを持つ。ツリー・Finder の候補（component/finder/index.rs）・FS 監視（watch.rs）の 3 者がこれを共有するのは、条件がずれると「ツリーには出るのに Finder に出ない」「表示しているのに自動リロードだけ効かない」が起きるため。bool を個別に配って回らない
- **無視ファイルの表示（`i`）は走査を切り替えるだけでなく「どれが無視対象か」も要る**（暗色で区別するため）。ignore クレートの走査結果にはその情報が無いので、`scan::read_dir` は同じ 1 階層を「無視を効かせた設定」でもう一度歩き、そちらに出てこなかったものを無視対象と見なす。パターンの解釈（否定・アンカー・祖先の .gitignore）を自前で持たず、表示・非表示と完全に同じ判定を使うのが目的。追加コストは **show_ignored が on の間だけ**の 1 階層ぶんの readdir 1 回で、無視されたディレクトリの配下は git 的にも全て無視対象なので `parent_ignored` を伝播させて再走査自体を省く
- 監視の開始（notify の再帰 watch 登録）も**ツリーの大きさに比例する**ため別スレッドに出し、`FsWatcher::drain` が毎 tick 受け取りに行く。登録完了までのイベントは取りこぼすが、それは監視開始前と同じ状態でしかない

## ツリー走査と FS 監視
- 走査は起動時に WalkBuilder 1 回で一括（サブディレクトリ起点の遅延走査だと親の .gitignore が効かない）。`require_git(false)` で非 git ディレクトリでも .gitignore を尊重
- `rescan` は展開状態と選択を **path で**保存・復元する（index_path は再走査で無効になる）
- **削除された（worktree または index で `D`）が未コミットのファイルは合成ノードとして Tree に足す**（`Tree::sync_deleted`）。WalkBuilder は実ファイルしか見ないため、`rm` 等で既に消えたパスは通常の走査に一切出てこず、このままでは GIT レーンで選択も stage/unstage もできない。Tree は本来 git を知らない設計だが、削除ファイルの可視化だけはこの橋渡しが無いと表現できないため例外的に許容する。`App::rescan` / `App::new` / `toggle_hidden` が nodes を作り直す（＝合成ノードも失う）都度、最新の git status から呼び直す設計で、専用の同期タイマーは作らない。**削除集合は `Tree` が持ち続け、実際の挿し込みは `rebuild_visible` が毎回行う** — 合成ノードを失うのは rescan だけでなく**遅延ロード（`scan::load` が実走査の結果で children を丸ごと置き換える）でも起きる**ため。「起動時に展開されていないディレクトリ配下の削除ファイルが、そのディレクトリを開いた瞬間に消える」という形で表面化していた（1 回挿して終わりにはできない）
- watch.rs のイベントフィルタは「`.` 始まり成分の除外 + root .gitignore の `matched_path_or_any_parents`」（`matched` だと `target/` が配下パスに効かない）。ツリー再走査は 500ms デバウンスで、git status の再取得もこれに相乗りする（別タイマーを作らない）
- **`FsWatcher::drain` はイベントを「構造変化 (作成・削除・リネーム)」と「内容だけの変更 (Modify(Data))」に分類して返す**（`watch::Change { path, structural }`）。ファイルの中身が変わってもツリーの行構成（どのパスが存在するか）は変わらないため、`App::on_tick` は structural なイベントが 1 件も無ければ `tree.rescan`（WalkBuilder の全走査）を丸ごとスキップし、`App::rescan_status_only`（git status の再取得 + GIT レーンの絞り込み・diff 更新だけ）で済ませる。大きい repo では「AI が高速に書き換え続ける」ような内容変更の連打が全走査の主なコストだったため、ここを削るのが効く。`Modify(Metadata)` は従来通り完全無視、種別が判別できない Modify は安全側 (structural) に倒す — 誤って全走査を省略し表示が古いまま固定される事故より、たまに余計な全走査をする方が無害なため
- **GIT レーンの絞り込み・diff は status ベースで足りる**ので、内容変更だけの tick でも `tree.set_filter`/`GitState::refresh` は毎回呼ぶ（`App::after_status_refresh`、rescan/rescan_status_only 共通）。「新しく変更されたファイルが絞り込みに現れる」という要求は `GitStatus.files`（`git status` の出力）だけで満たせ、ツリーの再走査は要らない。以前は「GIT レーンにいる間は変更が 1 件でもあれば無条件に rescan_pending を立てる」という特別扱いがあったが、この分類導入後は不要になった（全ての内容変更イベントが既に `after_status_refresh` を通るため）ので削除した
- 削除・作成・リネームは常に structural 扱いで `rescan()`（全走査）側に回るため、`tree.sync_deleted`（削除ファイルの合成ノード追加）は `rescan_status_only` では呼ばない。内容変更だけの tick では新しく削除されたパスが発生しない前提
- **「今開いているファイルの reload」と「再走査/status 再取得の保留フラグ」は排他にしない**（`App::on_tick` のイベント分類）。以前は `if 開いている path { viewer.reload } else if structural { .. } else { .. }` と繋がっていたため、**閲覧・編集中のファイルを書き換えても git status が再取得されず**、差分の有無に依存する表示（GIT レーンの可否 = `git_available`・ツリーの status・diff）が `r` を押すまで古いままだった。開いているかどうかは「viewer の cache を捨てるか」を決めるだけで、git 側の追従が要るかどうかとは独立している
- **fv 上での保存 (`Ctrl+s`) も FS 監視のイベント待ちにしない**。`EditState::save` が立てる take フラグ (`EditState::take_saved`) を `App::on_edit_key` が回収して `status_pending` を立てる（監視を張れない環境でも効かせるため）。ファイルの増減は起きないので全走査は要らず、再取得自体は `on_tick` の 500ms デバウンスに任せる（連続保存で git を連打しない）。EditState は App を借りられない（CLAUDE.md「閲覧と編集の関係」の依存範囲）ので、`EditBuffer::take_touched`（`Touched`）と同じ take フラグで橋渡しする

## Finder の候補（component/finder/index.rs）
ツリーが遅延走査になったので、`Ctrl+p` の候補をツリーから集めると未展開の階層が丸ごと欠ける。`FileIndex` が root 全体を**別スレッドで 1 回歩いて**候補を持つ（無視設定はツリーと同じ `ScanOptions::walker` を通すので、隠し項目・無視ファイルの表示切替がそのまま候補にも効く）。
- 走査を起こすのは Finder を開いた時だけ（起動時に走らせると、使わないのに巨大ディレクトリを歩くことになる）
- 走査完了前に開いた場合は**ツリーの読み込み済み分**で即座に開き、完了時に `on_tick` が `Finder::set_candidates` で差し替える（クエリは保つ）。タイトルの `scanning...` がその状態
- FS 変更・隠しファイル切替では `invalidate` するだけ。ここで走査し直すと保存のたびに全走査になる（古い一覧は次に Finder を開くまで使い続ける）

## ツリーのファイル操作（app/file_ops.rs、n/N/R/D/y）
ファイルの新規作成 (`n`)・ディレクトリ作成 (`N`)・リネーム (`R`)・削除 (`D`)・相対パスのコピー (`y`) を Focus::Tree で拾う（keys.rs の `on_file_op_key`。VIEW/GIT のどちらのレーンでも同じキーで効く — ツリー自体が共用なので分けない）。git を経由せず `std::fs` で直接書くので tracked/untracked を問わず同じ挙動になり、git 側の追従は他の書き込み系操作と同じ `rescan_now` に相乗りさせる（専用の同期パスを作らない）。
- 名前入力は `Mode::Input` に `InputKind::NewFile/NewDir/Rename` を足して乗せる。InputKind は Copy の識別子だけなので、パスを要する対象（作成先の親ディレクトリ・リネーム元）は `App.file_op: Option<FileOp>` が持ち、Input を開いた時に立て Esc/Enter で落とす。ステータスバーの接頭辞（`App::file_op_label`）には作成先ディレクトリを添える — 選択行がファイルの時どの階層へ入るのかが見えないため
- 作成先は「選択行がディレクトリならそれ、ファイルならその親、空のツリーなら root」。`a/b/c.rs` のような入力は途中のディレクトリごと作る（`create_dir_all` + `create_new`。存在チェックと作成の間に外から作られても上書きしない）。作ったファイルはそのまま右ペインに開く
- 入力は `validate_name` で root 配下に閉じる（`..`・絶対パスは拒否、末尾の `/` だけ黙って落とす。リネームは 1 要素だけ = 別ディレクトリへの移動にはしない）。字面の join だけでは途中の symlink がツリーの外を指す `link/new` を通してしまうので、書く直前に `contained` が「存在する最も深い祖先」を canonicalize して root と突き合わせる（まだ無い末尾は自分が作る実体なので解決不要）。既に存在する名前への作成・リネームは fs に触る前に notice で断る
- リネームは `rename_no_replace`: exists の確認と `fs::rename` の間に外から同名が作られると黙って置き換わるため、ファイルは `hard_link`（宛先があれば必ず失敗）+ 元の削除で原子的に移し、hard_link を持たない fs でだけ rename に落とす。ディレクトリは hard_link できないので rename のまま（空でないディレクトリへの rename は OS が拒否する）。UTF-8 でない名前は入力欄に出せず置換文字入りの別名に化けるので `R` の時点で断る
- GIT レーンでは `open_selected` が diff 側にしか届かないので、作成・リネームで開き直す時は `viewer.open` も直接呼んで VIEW に戻った時の表示を揃える
- 削除だけは `Mode::Confirm`（`ConfirmAction::Delete`）を経由する。git の discard と違い復元できないため。ディレクトリは `remove_dir_all` で配下ごと消す
- 開いているファイルが消えた/動いた時は右ペインも追従させる（削除は `Viewer::close`、リネームは新しいパスで `open_selected`。配下のファイルはプレフィックスを付け替える）。横断検索の一覧は構造が変わるので `grep.invalidate`、Finder の候補は rescan 側で無効化される
- 作成・リネーム後は `Tree::reveal` で祖先を開いてその行を選択する（再走査の後でないと新しいパスがツリーに無いので順序は固定）。GIT レーンの絞り込み中は untracked として git status に現れるぶんだけ見える

## ツリーペインの描画（component/tree/view.rs）
- **`ListItem` の組み立ては画面に映る行数に比例させる**（以前は `tree.visible` 全体に比例していた。展開済みの巨大なツリーで `j` を押しっぱなしにすると 1 回の再描画あたり `visible` 全件ぶんの `format!`/`Vec` 確保が走り、キー入力への追従が目に見えて遅れていた）。`ListState` の scroll/offset 管理は ratatui の `List` に任せず自前に持ち替えた（下記 A 案）。B 案（組み立て済み `Vec<ListItem>` をキャッシュし内容が変わった時だけ作り直す）も検討したが、A 案の方が「常に O(画面行数)」を型で保証できて strictly 強く、`List::new` が `Vec<ListItem>` を所有として消費する ratatui の API 上、キャッシュを毎フレーム使い回すにも結局クローンが要って B 案の優位性が薄れるため見送った
- ツリーの行は高さが常に 1 (`row.name` に改行は入らない) という前提があるので、ratatui `List` が内部でやる「選択行を含む最小限のウィンドウを保つ」スクロール計算 (`get_items_bounds`、非公開 API) は、offset を起点に selected が入るまで前後にスライドさせるだけの O(1) の式に厳密に置き換えられる（`component/tree/view.rs::visible_window`）。この式は ratatui 側のテストケース (`selected_item_ensures_selected_item_is_visible_when_offset_is_*`) の期待値と突き合わせて導出した。可変高さ行 (`repeat_highlight_symbol`・複数行アイテム等) は使っていないので、この前提が崩れる変更 (行を複数行にする等) をする時はこの等価性も一緒に見直すこと
- `[first, last)` の絶対 offset は `app.tree.list_state`（`offset_mut()`）に書き戻す。`app/mouse.rs::click_tree_row` がクリック行の絶対 index 換算にこの offset を読むため（`tree_area`/`viewport.height` などと同じ 描画→app の書き戻しパターン）。`List` 自体には `[first, last)` にスライスした部分列と、それに合わせて相対化した選択位置を持つ使い捨ての `ListState` を渡す — `List::new` が受け取った `Vec<ListItem>` をそのままインデックス 0 起点として扱うため、絶対値の `list_state` をそのまま渡すと選択位置も offset も二重にずれる
- 選択のハイライトは `List::highlight_style` が描画時に当てるだけで `ListItem` 自体には焼き込まれないため、`j`/`k` で選択が動くだけなら（＝ウィンドウの範囲が変わらなければ）以前と同じ行の `ListItem` を作り直しても意味が無い。今回のウィンドウ縮小と合わせて、実質的に「画面外の行は最初から作らない」形になっている

