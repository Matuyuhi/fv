use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Paragraph;

use crate::component::editor::EditState;
use crate::component::viewer::Viewer;
use crate::text;

use crate::widget::pane_block;
use crate::widget::text_pane::{LineWindow, TextPane};

// 編集中の右ペイン。描画パイプラインは閲覧と共通 (text_pane)。
// 検索ハイライトを持たず、代わりにブロックカーソルを重ねる点だけが違う
// EDIT レーンは Viewport (スクロール共有) と Highlighter を Viewer から借りる関係なので、
// 自分の状態に加えて Viewer も受け取る (component/editor/mod.rs の依存範囲と同じ)
pub(crate) fn draw_editor(
    frame: &mut Frame,
    state: &mut EditState,
    viewer: &mut Viewer,
    area: Rect,
) {
    viewer.viewport.height = area.height.saturating_sub(2) as usize;
    viewer.viewport.width = area.width.saturating_sub(2) as usize;
    let background = viewer.background();

    let name = viewer
        .current
        .as_ref()
        .map(|open| open.title.clone())
        .unwrap_or_else(|| state.path.display().to_string());
    let dirty = if state.buffer.dirty() { "*" } else { "" };
    // 編集はモーダルなのでフォーカスは常にこのペイン扱い
    let block = pane_block(format!("{name}{dirty} [EDIT]"), true);

    let (cursor_line, cursor_col) = state.cursor;
    let cursor_display = text::display_col(state.buffer.line(cursor_line), cursor_col);
    let gutter_width = state.gutter_width();
    // 編集中も可視範囲だけを組み立てる。直前の編集で無効化された行だけが実際に再計算される
    let (rows, first) =
        state
            .render
            .rows(&viewer.highlighter, state.buffer.source(), &viewer.viewport);
    let pane = TextPane {
        window: LineWindow { rows, first },
        changed_lines: &state.changed_lines,
        search: None,
        cursor: Some((cursor_line, cursor_display)),
        gutter_width,
    };
    let visible = pane.visible(&viewer.viewport);
    let paragraph = Paragraph::new(visible)
        .block(block)
        .style(Style::default().bg(background));
    frame.render_widget(paragraph, area);
}
