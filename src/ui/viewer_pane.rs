use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::Paragraph;

use crate::app::{App, Focus};
use crate::text;
use crate::viewer::Content;

use super::pane_block;
use super::text_pane::TextPane;

pub(super) fn draw_viewer(frame: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Viewer;
    // マウス・キー処理が次のフレームで読む実測値の書き戻し (ui→app 逆流の統一パターン)
    app.viewer.viewport.height = area.height.saturating_sub(2) as usize;
    app.viewer.viewport.width = area.width.saturating_sub(2) as usize;

    let Some(open) = &app.viewer.current else {
        let paragraph = Paragraph::new("no file selected")
            .block(pane_block(String::from("viewer"), focused))
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(paragraph, area);
        return;
    };
    // hscroll > 0 の間はステータスバーではなくタイトル側に現在オフセットを出す
    let title = if !app.viewer.viewport.wrap && app.viewer.viewport.hscroll > 0 {
        format!("{}  →{}", open.title, app.viewer.viewport.hscroll)
    } else {
        open.title.clone()
    };
    let block = pane_block(title, focused);
    match open.content.as_ref() {
        Content::Text { lines, .. } => {
            let pane = TextPane {
                lines,
                changed_lines: &open.changed_lines,
                search: app.viewer.search.as_ref(),
                cursor: None,
                gutter_width: text::gutter_width(lines.len()),
            };
            let visible = pane.visible(&app.viewer.viewport);
            let paragraph = Paragraph::new(visible)
                .block(block)
                .style(Style::default().bg(app.viewer.background()));
            frame.render_widget(paragraph, area);
        }
        Content::Binary => {
            let paragraph = Paragraph::new("binary file")
                .block(block)
                .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(paragraph, area);
        }
        Content::Error(message) => {
            let paragraph = Paragraph::new(message.as_str())
                .block(block)
                .style(Style::default().fg(Color::Red));
            frame.render_widget(paragraph, area);
        }
    }
}
