mod finder_panel;
mod help;
mod icons;
mod status_bar;
mod tree_pane;
mod viewer_pane;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders};

use crate::app::{App, Mode};

pub fn draw(frame: &mut Frame, app: &mut App) {
    let full = frame.area();
    let [main, status] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(full);
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(70)]).areas(main);
    // マウスのヒットテスト用に、次の on_mouse で使えるよう書き戻す (viewport_height と同じパターン)
    app.tree_area = left;
    app.viewer_area = right;
    tree_pane::draw_tree(frame, app, left);
    viewer_pane::draw_viewer(frame, app, right);
    status_bar::draw_status_bar(frame, app, status);
    if matches!(app.mode, Mode::Finder(_)) {
        finder_panel::draw_finder(frame, app, full);
    }
    if matches!(app.mode, Mode::Help) {
        help::draw_help(frame, full);
    }
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
