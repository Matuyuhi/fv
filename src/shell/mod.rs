//! 画面全体の骨格 (レイアウト・各 View への値の取り出し) と、App 全体の状態を横断して
//! 見せる画面。個々の部品は component/*/view.rs 側にあり、ここはそれを配置して必要な値を
//! 渡す合成の場に徹する。status_bar/settings/confirm/commit/tab_bar だけがここに置かれて
//! いるのは、どれも「App 全体を見せる」ことそのものが役目で、単一の状態型に閉じないため
//! (CLAUDE.md「描画の依存範囲」)。

mod commit;
mod confirm;
mod help;
mod settings;
mod status_bar;
mod tab_bar;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};

use crate::app::{App, Focus, Lane, Mode, Workspace};
use crate::component::{branch, editor, finder, gitlane, issues, log, prs, tree, viewer};

pub fn draw(frame: &mut Frame, app: &mut App) {
    let full = frame.area();
    // GitHub モードが使えない (既定) 間はタブバーの 1 行も確保しない。
    // 無効時の見た目を 1 ピクセルも変えないための唯一の分岐点
    let (tab_area, main, status) = if app.workspace_available() {
        let [tab, main, status] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .areas(full);
        (Some(tab), main, status)
    } else {
        let [main, status] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(full);
        (None, main, status)
    };
    if let Some(tab_area) = tab_area {
        tab_bar::draw_tab_bar(frame, app, tab_area);
    } else {
        // タブが出ない間はクリック判定の対象も無い (mouse.rs はここを読む)
        app.tab_areas = Default::default();
    }

    match app.workspace {
        Workspace::Viewer => draw_viewer_workspace(frame, app, main),
        Workspace::Issues => draw_issues_workspace(frame, app, main),
        Workspace::PullRequests => draw_pr_workspace(frame, app, main),
    }

    status_bar::draw_status_bar(frame, app, status);
    // 自分の状態だけで描けるオーバーレイ (Finder/Branch) は、その状態だけを渡す。
    // Help/Settings/Confirm/Commit は App 全体の設定・Mode の中身をそのまま見せる
    // 「シェル側の画面」なので &App のままにしてある (status_bar と同じ扱い)
    let scanning = app.file_index.scanning();
    if let Mode::Finder(finder) = &mut app.mode {
        finder::view::draw_finder(frame, finder, scanning, full);
    }
    if let Mode::Help { scroll } = app.mode {
        // 実測 (表示行数・総行数) を書き戻し、次フレームの on_help_key がクランプと
        // ページ送りに使う (viewport.height と同じ 描画→app のパターン)
        app.help_view = help::draw_help(frame, scroll, full);
    }
    if matches!(app.mode, Mode::Settings(_)) {
        settings::draw_settings(frame, app, full);
    }
    if matches!(app.mode, Mode::Confirm { .. }) {
        confirm::draw_confirm(frame, app, full);
    }
    if matches!(app.mode, Mode::Commit { .. }) {
        commit::draw_commit(frame, app, full);
    }
    if let Mode::Branch(state) = &mut app.mode {
        branch::view::draw_branch(frame, state, full);
    }
}

// Workspace::Viewer の中身。改名前の draw 本体そのまま (Lane 3 種 + ツリー + オーバーレイの
// 既存アプリ全体がここに入る)
fn draw_viewer_workspace(frame: &mut Frame, app: &mut App, main: Rect) {
    // 幅はドラッグで変わるので、割合ではなく App が持つ実桁数で切る
    let [left, right] = Layout::horizontal([
        Constraint::Length(app.tree_width(main.width)),
        Constraint::Min(1),
    ])
    .areas(main);
    // マウスのヒットテスト用に、次の on_mouse で使えるよう書き戻す (viewport の実測値と同じパターン)
    app.tree_area = left;
    app.viewer_area = right;
    // 掴み代を確保するため、隣接する枠線 2 桁 (左ペインの右枠 + 右ペインの左枠) を境界とする
    app.splitter_area = Rect {
        x: left.right().saturating_sub(1),
        y: main.y,
        width: 2.min(main.width),
        height: main.height,
    };
    // LOG は左ペインもツリーではなくコミット一覧に差し替わるため、他レーンより先に分岐して
    // 左右まとめて専用の描画へ渡す (tree_pane は呼ばない)
    if matches!(app.lane, Lane::Log(_)) {
        let list_focused = app.focus == Focus::Tree;
        let diff_focused = app.focus == Focus::Viewer;
        let background = app.viewer.background();
        if let Lane::Log(log) = &mut app.lane {
            log::view::draw_log_list(frame, log, list_focused, left);
            log::view::draw_log_diff(frame, log, diff_focused, background, right);
        }
        return;
    }
    tree::view::draw_tree(
        frame,
        &mut app.tree,
        app.git.as_ref(),
        &app.root,
        app.icons,
        app.focus == Focus::Tree,
        left,
    );
    // 右ペインの中身はレーンで決まる (VIEW: ファイル / EDIT: 編集バッファ / GIT: diff)。
    // どのレーンの描画も「そのレーンの状態 + 必要なスカラ」しか受け取らない — App 全体を
    // 渡さないことで、View が触れる状態の範囲を型で縛る
    let focused = app.focus == Focus::Viewer;
    if let Lane::Edit(edit) = &mut app.lane {
        // EDIT は Viewport (スクロール共有) と Highlighter を Viewer から借りる関係なので
        // Viewer も渡す (app.lane と app.viewer は互いに素なフィールドなので同時に借りられる)
        editor::view::draw_editor(frame, edit, &mut app.viewer, right);
    } else if matches!(app.lane, Lane::Git(_)) {
        // GitState は app.lane の中にあるので、先に必要な値を取り出してから借りる
        let background = app.viewer.background();
        if let Lane::Git(git) = &mut app.lane {
            gitlane::view::draw_git(frame, git, focused, background, right);
        }
    } else {
        viewer::view::draw_viewer(frame, &mut app.viewer, focused, right);
    }
}

// issues タブ (#33) の中身。左 = 一覧、右 = 詳細で、幅・ドラッグリサイズは Viewer タブと
// 同じ App::tree_width / split_ratio を共有する (tree_area 等の書き戻しも同じパターン)
fn draw_issues_workspace(frame: &mut Frame, app: &mut App, main: Rect) {
    let [left, right] = Layout::horizontal([
        Constraint::Length(app.tree_width(main.width)),
        Constraint::Min(1),
    ])
    .areas(main);
    app.tree_area = left;
    app.viewer_area = right;
    app.splitter_area = Rect {
        x: left.right().saturating_sub(1),
        y: main.y,
        width: 2.min(main.width),
        height: main.height,
    };
    let list_focused = app.focus == Focus::Tree;
    let detail_focused = app.focus == Focus::Viewer;
    let background = app.viewer.background();
    issues::view::draw_issues_list(frame, &mut app.issues, list_focused, left);
    issues::view::draw_issues_detail(frame, &mut app.issues, detail_focused, background, right);
}

// pull requests タブ (#34) の中身。issues タブと同じ左右分割・幅共有パターン
// (App::tree_width / split_ratio、tree_area 等の書き戻し)
fn draw_pr_workspace(frame: &mut Frame, app: &mut App, main: Rect) {
    let [left, right] = Layout::horizontal([
        Constraint::Length(app.tree_width(main.width)),
        Constraint::Min(1),
    ])
    .areas(main);
    app.tree_area = left;
    app.viewer_area = right;
    app.splitter_area = Rect {
        x: left.right().saturating_sub(1),
        y: main.y,
        width: 2.min(main.width),
        height: main.height,
    };
    let list_focused = app.focus == Focus::Tree;
    let detail_focused = app.focus == Focus::Viewer;
    let background = app.viewer.background();
    prs::view::draw_pr_list(frame, &mut app.prs, list_focused, left);
    prs::view::draw_pr_detail(frame, &mut app.prs, detail_focused, background, right);
}
