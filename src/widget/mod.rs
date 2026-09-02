//! 複数のコンポーネントが共有する描画部品。ここには「どの状態を描くか」を持たせず、
//! 渡された Line 列・文字列をどう見せるかだけを担当させる。

pub mod diff_boundary;
pub mod icons;
pub mod text_pane;

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders};

/// ratatui `List` が内部で行う「選択行を含む最小限のウィンドウ」計算 (get_items_bounds) と
/// 等価な結果を返す。ツリーもコミット一覧も行は全て高さ 1 (行の文字列に改行が入らない) なので、
/// あちらのような可変高さ対応のループは要らず、offset を起点に selected が入るまでスライドする
/// だけの O(1) 計算に落とせる。selected が既にウィンドウ内なら offset をそのまま保つのがポイントで、
/// ここを毎回 selected 中心に作り直すと「選択が動くたびに画面が揺れる」挙動になってしまう。
/// **呼び出し側は必ずこの範囲だけ ListItem を組む** — 一覧全体に比例させると、行数が増えるほど
/// 1 打鍵あたりの再描画が重くなる (CLAUDE.md「再描画のコストを画面の大きさより上に持ち上げない」)。
/// 複数行アイテム等で行の高さが 1 でなくなる変更をする時は、この等価性も一緒に見直すこと
pub(crate) fn visible_window(
    total: usize,
    max_height: usize,
    offset: usize,
    selected: Option<usize>,
) -> (usize, usize) {
    let offset = offset.min(total - 1);
    let index_to_display = selected.map(|s| s.min(total - 1)).unwrap_or(offset);
    let mut first = offset;
    let mut last = (offset + max_height).min(total);
    if index_to_display >= last {
        first = index_to_display + 1 - max_height.min(index_to_display + 1);
        last = (first + max_height).min(total);
    } else if index_to_display < first {
        first = index_to_display;
        last = (first + max_height).min(total);
    }
    (first, last)
}

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

pub(crate) fn center_text(text: &str, height: u16) -> String {
    let lines = text.lines().count() as u16;
    let padding = height.saturating_sub(lines) / 2;
    if padding == 0 {
        return text.to_string();
    }
    format!("{}{}", "\n".repeat(padding as usize), text)
}
