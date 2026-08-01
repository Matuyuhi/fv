use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Paragraph;

use crate::app::{App, Lane};
use crate::text;

use super::pane_block;
use super::text_pane::{LineWindow, TextPane};

// 編集中の右ペイン。描画パイプラインは閲覧と共通 (text_pane)。
// 検索ハイライトを持たず、代わりにブロックカーソルを重ねる点だけが違う
pub(super) fn draw_editor(frame: &mut Frame, app: &mut App, area: Rect) {
    app.viewer.viewport.height = area.height.saturating_sub(2) as usize;
    app.viewer.viewport.width = area.width.saturating_sub(2) as usize;
    let background = app.viewer.background();
    // state を可変で借りる前に Viewer 側から要る値を取り出しておく
    let open_title = app.viewer.current.as_ref().map(|open| open.title.clone());

    let Lane::Edit(state) = &mut app.lane else {
        return;
    };
    let name = open_title.unwrap_or_else(|| state.path.display().to_string());
    let dirty = if state.buffer.dirty() { "*" } else { "" };
    // 編集はモーダルなのでフォーカスは常にこのペイン扱い
    let block = pane_block(format!("{name}{dirty} [EDIT]"), true);

    let (cursor_line, cursor_col) = state.cursor;
    let cursor_display = text::display_col(state.buffer.line(cursor_line), cursor_col);
    let gutter_width = state.gutter_width();
    // 編集中も可視範囲だけを組み立てる。直前の編集で無効化された行だけが実際に再計算される
    let (rows, first) = state.render.rows(
        &app.viewer.highlighter,
        state.buffer.source(),
        &app.viewer.viewport,
    );
    let pane = TextPane {
        window: LineWindow { rows, first },
        changed_lines: &state.changed_lines,
        search: None,
        cursor: Some((cursor_line, cursor_display)),
        gutter_width,
    };
    let visible = pane.visible(&app.viewer.viewport);
    let paragraph = Paragraph::new(visible)
        .block(block)
        .style(Style::default().bg(background));
    frame.render_widget(paragraph, area);
}
