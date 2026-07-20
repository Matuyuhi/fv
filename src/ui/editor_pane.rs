use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Paragraph;

use crate::app::{App, Mode};
use crate::text;

use super::pane_block;
use super::text_pane::TextPane;

// 編集中の右ペイン。描画パイプラインは閲覧と共通 (text_pane)。
// 検索ハイライトを持たず、代わりにブロックカーソルを重ねる点だけが違う
pub(super) fn draw_editor(frame: &mut Frame, app: &mut App, area: Rect) {
    app.viewer.viewport.height = area.height.saturating_sub(2) as usize;
    app.viewer.viewport.width = area.width.saturating_sub(2) as usize;

    let Mode::Edit(state) = &app.mode else {
        return;
    };
    let name = app
        .viewer
        .current
        .as_ref()
        .map(|open| open.title.clone())
        .unwrap_or_else(|| state.path.display().to_string());
    let dirty = if state.buffer.dirty() { "*" } else { "" };
    // 編集はモーダルなのでフォーカスは常にこのペイン扱い
    let block = pane_block(format!("{name}{dirty} [EDIT]"), true);

    let (cursor_line, cursor_col) = state.cursor;
    let cursor_display = text::display_col(state.buffer.line(cursor_line), cursor_col);
    let pane = TextPane {
        lines: &state.lines,
        changed_lines: &state.changed_lines,
        search: None,
        cursor: Some((cursor_line, cursor_display)),
        gutter_width: state.gutter_width,
    };
    let visible = pane.visible(&app.viewer.viewport);
    let paragraph = Paragraph::new(visible)
        .block(block)
        .style(Style::default().bg(app.viewer.background()));
    frame.render_widget(paragraph, area);
}
