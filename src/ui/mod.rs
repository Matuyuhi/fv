mod commit;
mod confirm;
mod diff_boundary;
mod editor_pane;
mod finder_panel;
mod git_pane;
mod help;
mod icons;
mod log_pane;
mod settings_panel;
mod status_bar;
mod tab_bar;
mod text_pane;
mod tree_pane;
mod viewer_pane;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::{App, Focus, Lane, Mode, Workspace};

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

    if matches!(app.workspace, Workspace::Viewer) {
        draw_viewer_workspace(frame, app, main);
    } else {
        // Issues/PR は #33/#34 までプレースホルダ。ツリー/ビューアのクリック対象を
        // 残すと誤ヒットするので空にしておく
        app.tree_area = Rect::default();
        app.viewer_area = Rect::default();
        app.splitter_area = Rect::default();
        draw_workspace_placeholder(frame, app, main);
    }

    status_bar::draw_status_bar(frame, app, status);
    if matches!(app.mode, Mode::Finder(_)) {
        finder_panel::draw_finder(frame, app, full);
    }
    if matches!(app.mode, Mode::Help) {
        help::draw_help(frame, full);
    }
    if matches!(app.mode, Mode::Settings(_)) {
        settings_panel::draw_settings(frame, app, full);
    }
    if matches!(app.mode, Mode::Confirm { .. }) {
        confirm::draw_confirm(frame, app, full);
    }
    if matches!(app.mode, Mode::Commit { .. }) {
        commit::draw_commit(frame, app, full);
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
            log_pane::draw_log_list(frame, log, list_focused, left);
            log_pane::draw_log_diff(frame, log, diff_focused, background, right);
        }
        return;
    }
    tree_pane::draw_tree(frame, app, left);
    // 右ペインの中身はレーンで決まる (VIEW: ファイル / EDIT: 編集バッファ / GIT: diff)
    if matches!(app.lane, Lane::Edit(_)) {
        editor_pane::draw_editor(frame, app, right);
    } else if matches!(app.lane, Lane::Git(_)) {
        // GitState は app.lane の中にあるので、先に必要な値を取り出してから借りる
        let focused = app.focus == Focus::Viewer;
        let background = app.viewer.background();
        if let Lane::Git(git) = &mut app.lane {
            git_pane::draw_git(frame, git, focused, background, right);
        }
    } else {
        viewer_pane::draw_viewer(frame, app, right);
    }
}

// Issues/PullRequests タブの中身。#33/#34 まで空のプレースホルダで良い (issue #32 のスコープ)
fn draw_workspace_placeholder(frame: &mut Frame, app: &App, area: Rect) {
    let title = Workspace::LABELS[app.workspace.index()].to_string();
    let paragraph = Paragraph::new("準備中 (#33 / #34 で実装予定)")
        .block(pane_block(title, true))
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(paragraph, area);
}

fn pane_block(title: String, focused: bool) -> Block<'static> {
    let border_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title)
}

// 画面中央に percent_x% x percent_y% のオーバーレイ領域を切り出す
// 高さだけ実寸で指定する版。中身の行数が決まっているオーバーレイ (確認ダイアログ) は
// 割合で切ると低い端末で肝心の行が落ちるため、行数から高さを決められるようにする
fn centered_rect_with_height(percent_x: u16, height: u16, area: Rect) -> Rect {
    let margin = area.height.saturating_sub(height) / 2;
    let [_, middle, _] = Layout::vertical([
        Constraint::Length(margin),
        Constraint::Length(height),
        Constraint::Min(0),
    ])
    .areas(area);
    let [_, center, _] = Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .areas(middle);
    center
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let [_, middle, _] = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ])
    .areas(area);
    let [_, center, _] = Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .areas(middle);
    center
}
