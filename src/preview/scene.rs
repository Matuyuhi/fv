//! プレビューするシーンの一覧。1 シーン = 「この状態の画面が見たい」を 1 つ書いたもので、
//! Compose の `@Preview` 関数 1 つに相当する。状態の作り方は原則キー列 (preview/keys.rs) で、
//! キーでは辿り着けないもの (gh の応答待ちが要る issues/PR タブ) だけ状態を直接注入する。

use std::path::PathBuf;
use std::sync::mpsc;

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::app::{App, Lane};
use crate::component::prs::{self, DetailView};
use crate::github::{PrRow, RemoteItem};

use super::keys;

pub struct Scene {
    pub name: &'static str,
    pub description: &'static str,
    /// 既定サイズ以外で見たいシーンだけ指定する (--size 指定時はそちらが優先)
    pub size: Option<(u16, u16)>,
    /// 1 回目の描画 (viewport の実測値が App に書き戻された後) に呼ばれる。
    /// height に依存するキー (Ctrl+d 等) を測定前に流すとシーンごとに結果がぶれるため、
    /// 「描画 → setup → 描画」の順序は preview/mod.rs 側で固定してある
    pub setup: fn(&mut App),
}

pub const SCENES: &[Scene] = &[
    Scene {
        name: "tree",
        description: "ツリーだけ (ファイル未選択)",
        size: None,
        setup: |app| {
            expand(app, "src");
        },
    },
    Scene {
        name: "tree-ignored",
        description: "ツリー: i で .gitignore 対象も表示 (暗色)",
        size: None,
        setup: |app| {
            send(app, "i");
            expand(app, "target");
        },
    },
    Scene {
        name: "view",
        description: "VIEW レーン: ファイルを開いた既定の画面",
        size: None,
        setup: |app| {
            open(app, "src/main.rs");
        },
    },
    Scene {
        name: "wrap",
        description: "VIEW レーン: 折返し表示 (w) — 続き行の gutter pad",
        // 折返しが実際に起きる幅でないと意味がないので、narrow と同じ狭い端末で撮る
        size: Some((64, 18)),
        setup: |app| {
            open(app, "src/main.rs");
            send(app, "<Tab>w");
        },
    },
    Scene {
        name: "view-binary",
        description: "VIEW レーン: 非テキストファイルのフォールバック",
        size: None,
        setup: |app| {
            open(app, "assets/logo.bin");
        },
    },
    Scene {
        name: "search",
        description: "VIEW レーン: 検索ハイライトと n/N の状態",
        size: None,
        setup: |app| {
            open(app, "src/main.rs");
            send(app, "<Tab>/app<CR>n");
        },
    },
    Scene {
        name: "select",
        description: "VIEW レーン: 行単位の範囲選択 (v → j) とコピーのヒント",
        size: None,
        setup: |app| {
            open(app, "src/main.rs");
            // マウスのドラッグはキー列で表せないので、キーボード側の入口 (v) で撮る
            send(app, "<Tab>vjjjj");
        },
    },
    Scene {
        name: "edit",
        description: "EDIT レーン: カーソル + 未保存バッファのライブ diff",
        size: None,
        setup: |app| {
            open(app, "src/main.rs");
            // EDIT レーンは印字キーを全て文字入力にするので、カーソル移動は矢印キーで行う
            send(app, "<Tab>e");
            send(
                app,
                "<Down><Down><Down><Down><Down><Down><Down><Down><Down>",
            );
            send(app, "<Right><Right><Right><Right>// 編集中の行");
        },
    },
    Scene {
        name: "git",
        description: "GIT レーン: 単一ファイルの inline diff (word-level 強調)",
        size: None,
        setup: |app| {
            to_lane(app, 2);
            open(app, "src/main.rs");
        },
    },
    Scene {
        name: "git-lines",
        description: "GIT レーン: 行カーソルと V の行単位選択 (Enter で行だけ stage)",
        size: None,
        setup: |app| {
            to_lane(app, 2);
            open(app, "src/main.rs");
            // diff ペインへ移り、変更行までカーソルを下ろしてから V で範囲を掴む
            send(app, "<Tab>jjjjVj");
        },
    },
    Scene {
        name: "git-side",
        description: "GIT レーン: side-by-side diff",
        size: Some((140, 32)),
        setup: |app| {
            to_lane(app, 2);
            open(app, "src/main.rs");
            send(app, "<Tab>v");
        },
    },
    Scene {
        name: "git-all",
        description: "GIT レーン: 全ファイルまとめ diff (sticky header 付き)",
        size: None,
        setup: |app| {
            to_lane(app, 2);
            open(app, "src/main.rs");
            send(app, "<Tab>A");
        },
    },
    Scene {
        name: "log",
        description: "LOG レーン: コミット一覧 + 選択コミットの diff",
        size: None,
        setup: |app| {
            to_lane(app, 3);
            send(app, "<CR>");
        },
    },
    Scene {
        name: "finder",
        description: "Ctrl+p ファジーファインダー",
        size: None,
        setup: |app| {
            open(app, "src/main.rs");
            send(app, "<C-p>ui");
        },
    },
    Scene {
        name: "help",
        description: "? ヘルプオーバーレイ",
        size: None,
        setup: |app| send(app, "?"),
    },
    Scene {
        name: "settings",
        description: "s 設定オーバーレイ",
        size: None,
        setup: |app| send(app, "sjj"),
    },
    Scene {
        name: "commit",
        description: "c コミットメッセージ入力 (50/72 桁ルーラー付き)",
        size: None,
        setup: |app| {
            send(app, "c");
            send(app, "feat: プレビュー機能を追加する<CR>");
            send(app, "実装しながら UI を確認できるようにする。");
        },
    },
    Scene {
        name: "branch",
        description: "b ブランチ一覧オーバーレイ",
        size: None,
        setup: |app| send(app, "b"),
    },
    Scene {
        name: "confirm",
        description: "X 破棄の確認オーバーレイ",
        size: None,
        setup: |app| {
            to_lane(app, 2);
            open(app, "src/main.rs");
            send(app, "X");
        },
    },
    Scene {
        name: "issues",
        description: "issues タブ (GitHub モード)",
        size: None,
        setup: |app| {
            send(app, "<A-2>");
            let rows = sample_issues();
            let number = rows[1].number;
            let (tx, rx) = mpsc::channel();
            let _ = tx.send(Ok(rows));
            app.issues.begin_list_fetch(rx);
            app.issues.poll();
            app.issues.move_selection(1);
            app.issues.request_open(number);
            let (tx, rx) = mpsc::channel();
            let _ = tx.send((number, Ok(sample_comments())));
            app.issues.begin_comments_fetch(rx);
            app.issues.poll();
        },
    },
    Scene {
        name: "prs",
        description: "pull requests タブ (GitHub モード)",
        size: None,
        setup: |app| {
            send(app, "<A-3>");
            let rows = sample_prs();
            let number = rows[0].item.number;
            let (tx, rx) = mpsc::channel();
            let _ = tx.send(Ok(rows));
            app.prs.begin_list_fetch(rx);
            app.prs.poll();
            app.prs.set_open(number, DetailView::Description);
            app.prs.request_current();
            let (tx, rx) = mpsc::channel();
            let _ = tx.send((number, Ok(sample_comments())));
            app.prs.begin_comments_fetch(rx);
            app.prs.poll();
        },
    },
    Scene {
        name: "prs-diff",
        description: "pull requests タブ: 差分表示 (d) と行カーソル",
        size: None,
        setup: |app| {
            send(app, "<A-3>");
            let rows = sample_prs();
            let number = rows[0].item.number;
            let (tx, rx) = mpsc::channel();
            let _ = tx.send(Ok(rows));
            app.prs.begin_list_fetch(rx);
            app.prs.poll();
            // 差分は一覧に含まれないデータなので、gh を叩かずに同じ画面を作るには
            // 取得結果の側から注入するしかない (説明・コメントと同じ形)
            app.prs.set_open(number, DetailView::Diff);
            let (tx, rx) = mpsc::channel();
            let _ = tx.send((number, Ok(prs::build_diff_data(sample_pr_diff()))));
            app.prs.begin_diff_fetch(rx);
            app.prs.poll();
            send(app, "<Tab>jjjjjj");
        },
    },
    Scene {
        name: "narrow",
        description: "狭い端末 (列が落ちる閾値の確認)",
        size: Some((64, 18)),
        setup: |app| {
            open(app, "src/main.rs");
        },
    },
];

pub fn find(name: &str) -> Option<&'static Scene> {
    SCENES.iter().find(|scene| scene.name == name)
}

fn send(app: &mut App, script: &str) {
    for key in keys::parse(script) {
        app.on_key(key);
    }
}

// Shift+Tab の循環でレーンを合わせる。直接 lane を差し替えると enter_git/enter_log の
// 初期化 (絞り込み・フォーカス寄せ) を飛ばしてしまい、実際の画面と食い違う
fn to_lane(app: &mut App, index: usize) {
    for _ in 0..Lane::LABELS.len() {
        if app.lane.index() == index {
            return;
        }
        send(app, "<S-Tab>");
    }
}

fn open(app: &mut App, rel: &str) {
    let path = select(app, rel);
    app.open_selected(&path);
}

fn expand(app: &mut App, rel: &str) {
    select(app, rel);
    app.tree.expand_or_enter();
}

// ツリー上で相対パスを選択する。途中のディレクトリは遅延走査なので、
// 1 階層ずつ開きながら降りる (実際の l キー操作と同じ経路)
fn select(app: &mut App, rel: &str) -> PathBuf {
    let mut path = app.root.clone();
    let components: Vec<&str> = rel.split('/').collect();
    for (i, component) in components.iter().enumerate() {
        path = path.join(component);
        let Some(index) = app.tree.visible.iter().position(|row| row.path == path) else {
            break;
        };
        app.tree.selected = index;
        if i + 1 < components.len() {
            app.tree.expand_or_enter();
        }
    }
    path
}

fn sample_issues() -> Vec<RemoteItem> {
    vec![
        issue(
            41,
            "GIT レーンで hunk 単位の stage をしたい",
            "matuyuhi",
            "2026-07-28T09:12:00Z",
            &["enhancement", "git"],
            "open",
            "diff を見ながら hunk ごとに stage できると、コミットを分けるのが楽になる。",
        ),
        issue(
            39,
            "実装しながら UI をプレビューしたい",
            "matuyuhi",
            "2026-07-30T22:05:00Z",
            &["enhancement", "dx"],
            "open",
            "Compose の @Preview / SwiftUI Preview のように、TUI を起動せず\n各状態の画面を一覧で確認したい。\n\n- レーン・オーバーレイごとに 1 シーン\n- 保存のたびに描き直す",
        ),
        issue(
            35,
            "巨大な diff で描画が固まる",
            "reviewer",
            "2026-07-21T04:44:00Z",
            &["bug"],
            "closed",
            "20000 行を超える diff で操作が重くなる。",
        ),
    ]
}

fn sample_prs() -> Vec<PrRow> {
    vec![
        PrRow {
            item: issue(
                62,
                "feat: 実装しながら UI を確認できるプレビューを追加する",
                "matuyuhi",
                "2026-08-01T01:20:00Z",
                &["enhancement"],
                "open",
                "`fv --preview` で、レーン・オーバーレイごとの画面を TUI を起動せずに描き出す。\n\n- TestBackend で 1 フレーム描画 → ANSI に落とす\n- 状態はキー列で組み立てる",
            ),
            head_ref: "claude/ui-preview".to_string(),
            is_draft: false,
        },
        PrRow {
            item: issue(
                61,
                "refactor: 肥大化した keys.rs を責務ごとに分割する",
                "matuyuhi",
                "2026-07-29T13:02:00Z",
                &["refactor"],
                "merged",
                "keys.rs はキーの振り分けだけを持つようにする。",
            ),
            head_ref: "claude/split-keys".to_string(),
            is_draft: false,
        },
        PrRow {
            item: issue(
                58,
                "perf: ツリーペインの ListItem 生成を画面行数に比例させる",
                "reviewer",
                "2026-07-24T08:31:00Z",
                &["perf"],
                "open",
                "巨大なツリーで j を押しっぱなしにすると追従が遅れる。",
            ),
            head_ref: "perf/tree-items".to_string(),
            is_draft: true,
        },
    ]
}

fn issue(
    number: u64,
    title: &str,
    author: &str,
    updated_at: &str,
    labels: &[&str],
    state: &str,
    body: &str,
) -> RemoteItem {
    RemoteItem {
        number,
        title: title.to_string(),
        author: author.to_string(),
        updated_at: updated_at.to_string(),
        labels: labels.iter().map(|l| l.to_string()).collect(),
        state: state.to_string(),
        body: body.to_string(),
    }
}

// PR 差分ペレビュー用の固定 diff。render_commit にそのまま通す (実物と同じ組み立て)
fn sample_pr_diff() -> String {
    [
        "diff --git a/src/preview/mod.rs b/src/preview/mod.rs",
        "index 1111111..2222222 100644",
        "--- a/src/preview/mod.rs",
        "+++ b/src/preview/mod.rs",
        "@@ -12,7 +12,10 @@ pub fn run(options: Options) -> Result<(), Box<dyn Error>> {",
        "     let selected = resolve_scenes(&options.scenes)?;",
        "     let root = fixture::build()?;",
        " ",
        "-    for scene in selected {",
        "+    // 実測値が App へ書き戻ってからキー列を流す (描画 → setup → 描画)",
        "+    for scene in &selected {",
        "         let buffer = draw_scene(scene, &root, size);",
        "+        let body = render::buffer_lines(&buffer, color);",
        "+        write!(out, \"{body}\")?;",
        "     }",
        "     Ok(())",
        " }",
    ]
    .join("\n")
}

fn sample_comments() -> Vec<Line<'static>> {
    [
        ("@reviewer", Color::Cyan),
        ("シーンはキー列で組み立てる形にしましょう。", Color::Reset),
        ("", Color::Reset),
        ("@matuyuhi", Color::Cyan),
        (
            "そうします。保存のたびに描き直せるようにもします。",
            Color::Reset,
        ),
    ]
    .into_iter()
    .map(|(text, color)| Line::from(Span::styled(text, Style::default().fg(color))))
    .collect()
}
