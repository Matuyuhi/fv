//! 複数のコンポーネントが共有する描画部品。ここには「どの状態を描くか」を持たせず、
//! 渡された Line 列・文字列をどう見せるかだけを担当させる。

pub mod diff_boundary;
pub mod icons;
pub mod text_pane;

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders};

pub(crate) fn pane_block(title: String, focused: bool) -> Block<'static> {
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
pub(crate) fn centered_rect_with_height(percent_x: u16, height: u16, area: Rect) -> Rect {
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

pub(crate) fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
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
