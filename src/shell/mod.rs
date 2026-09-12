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
use crate::component::{branch, editor, finder, gitlane, grep, issues, log, prs, tree, viewer};

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
    if matches!(app.mode, Mode::Grep) {
        grep::view::draw_grep(frame, &mut app.grep, full);
    }
    if let Mode::Help { scroll } = app.mode {
        // 実測 (表示行数・総行数) を書き戻し、次フレームの on_help_key がクランプと
        // ページ送りに使う (viewport.height と同じ 描画→app のパターン)
        let view = help::draw_help(frame, app, scroll, full);
        app.help_view = view;
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
    // 左ペインはコミット一覧パネル (`L`) を出している間だけ上下に割る。ツリーが上・履歴が下で、
    // 高さの換算は App::log_pane_height 1 箇所に閉じる (tree_width と同じパターン)
    let (tree_rect, log_rect) = if app.log_panel_visible() {
        let log_height = app.log_pane_height(left.height);
        let [tree_rect, log_rect] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(log_height)]).areas(left);
        (tree_rect, Some(log_rect))
    } else {
        (left, None)
    };
    // マウスのヒットテスト用に、次の on_mouse で使えるよう書き戻す (viewport の実測値と同じパターン)
    app.tree_area = tree_rect;
    app.log_area = log_rect.unwrap_or_default();
    app.viewer_area = right;
    // 掴み代を確保するため、隣接する枠線 2 桁 (左ペインの右枠 + 右ペインの左枠) を境界とする
    app.splitter_area = Rect {
        x: left.right().saturating_sub(1),
        y: main.y,
        width: 2.min(main.width),
        height: main.height,
    };
    tree::view::draw_tree(
        frame,
        &mut app.tree,
        app.git.as_ref(),
        &app.root,
        app.icons,
        app.focus == Focus::Tree,
        tree_rect,
    );
    if let Some(log_rect) = log_rect {
        let focused = app.focus == Focus::Log;
        if let Some(log) = &mut app.log {
            log::view::draw_log_list(frame, log, focused, log_rect);
        }
    }
    // 右ペインの中身はレーンで決まる (VIEW: ファイル or コミット diff / EDIT: 編集バッファ /
    // GIT: diff)。
    // どのレーンの描画も「そのレーンの状態 + 必要なスカラ」しか受け取らない — App 全体を
    // 渡さないことで、View が触れる状態の範囲を型で縛る
    let focused = app.focus == Focus::Viewer;
    // 右ペインは「最後に開いたもの」で決まる。コミットを開いていればファイルより優先して
    // その diff を出す (判定は App::showing_commit_diff 1 箇所)
    if app.showing_commit_diff() {
        let background = app.viewer.background();
        if let Some(log) = &mut app.log {
            log::view::draw_log_diff(frame, log, focused, background, right);
        }
        return;
    }
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

// issues / pull requests タブは「一覧だけ (全幅)」と「レール + 詳細」の 2 レイアウトを行き来する。
// 詳細を開いていない間に空の詳細ペインを出しておくと、タブに入った直後の主操作 (一覧から選ぶ)
// に対して画面の半分が常に無駄になるため。開いている間も一覧を消さないのは、レールから
// j/k + Enter で次を開けるようにするため (Viewer タブのツリーと同じ役割)
const RAIL_WIDTH: u16 = 24;

// 一覧ペイン (全幅 or レール) と詳細ペインの矩形を決め、マウスのヒットテスト用に書き戻す。
// issues/PR で同じなので 1 箇所に閉じる。ペイン境界のドラッグはどちらのレイアウトでも
// 持たない (レールは固定幅、一覧のみの時は境界そのものが無い) ので splitter_area は空にする
fn remote_layout(app: &mut App, main: Rect, detail_open: bool) -> (Rect, Option<Rect>) {
    app.splitter_area = Rect::default();
    if !detail_open {
        app.tree_area = main;
        // 空 Rect は contains が常に false なので、詳細ペインへのクリック・ホイールは
        // 判定を分岐させなくても自然に届かなくなる (tab_areas の無効化と同じ手)
        app.viewer_area = Rect::default();
        return (main, None);
    }
    // 狭い端末でレールが画面の 1/3 を超えないようにするだけの上限 (下限は 8 桁 = "#1234" が入る幅)
    let rail = RAIL_WIDTH.min((main.width / 3).max(8)).min(main.width);
    let [left, right] =
        Layout::horizontal([Constraint::Length(rail), Constraint::Min(1)]).areas(main);
    app.tree_area = left;
    app.viewer_area = right;
    (left, Some(right))
}

// issues タブの中身
fn draw_issues_workspace(frame: &mut Frame, app: &mut App, main: Rect) {
    let (list_rect, detail_rect) = remote_layout(app, main, app.issues.open_number().is_some());
    let list_focused = app.focus == Focus::Tree;
    issues::view::draw_issues_list(
        frame,
        &mut app.issues,
        list_focused,
        detail_rect.is_some(),
        list_rect,
    );
    if let Some(detail_rect) = detail_rect {
        let detail_focused = app.focus == Focus::Viewer;
        let background = app.viewer.background();
        issues::view::draw_issues_detail(
            frame,
            &mut app.issues,
            detail_focused,
            background,
            detail_rect,
        );
    }
}

// pull requests タブの中身。issues タブと同じ 2 レイアウト
fn draw_pr_workspace(frame: &mut Frame, app: &mut App, main: Rect) {
    let (list_rect, detail_rect) = remote_layout(app, main, app.prs.open_number().is_some());
    let list_focused = app.focus == Focus::Tree;
    prs::view::draw_pr_list(
        frame,
        &mut app.prs,
        list_focused,
        detail_rect.is_some(),
        list_rect,
    );
    if let Some(detail_rect) = detail_rect {
        let detail_focused = app.focus == Focus::Viewer;
        let background = app.viewer.background();
        prs::view::draw_pr_detail(frame, &mut app.prs, detail_focused, background, detail_rect);
    }
}
