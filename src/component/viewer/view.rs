use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::Paragraph;

use crate::component::viewer::{Content, Viewer};
use crate::text;

use crate::widget::pane_block;
use crate::widget::text_pane::{LineWindow, TextPane};

pub(crate) fn draw_viewer(frame: &mut Frame, viewer: &mut Viewer, focused: bool, area: Rect) {
    // マウス・キー処理が次のフレームで読む実測値の書き戻し (ui→app 逆流の統一パターン)
    viewer.viewport.height = area.height.saturating_sub(2) as usize;
    viewer.viewport.width = area.width.saturating_sub(2) as usize;
    // 描画行の組み立て中は viewer.render を可変で借りたままになるので、Viewer 全体を
    // 借りるメソッド (background) はその前に済ませておく
    let background = viewer.background();

    let Some(open) = &viewer.current else {
        let paragraph = Paragraph::new("no file selected")
            .block(pane_block(String::from("viewer"), focused))
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(paragraph, area);
        return;
    };
    // hscroll > 0 の間はステータスバーではなくタイトル側に現在オフセットを出す
    let title = if !viewer.viewport.wrap && viewer.viewport.hscroll > 0 {
        format!("{}  →{}", open.title, viewer.viewport.hscroll)
    } else {
        open.title.clone()
    };
    let block = pane_block(title, focused);
    let doc = match open.content.as_ref() {
        Content::Text(doc) => doc,
        Content::Binary => {
            let paragraph = Paragraph::new("binary file")
                .block(block)
                .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(paragraph, area);
            return;
        }
        Content::Error(message) => {
            let paragraph = Paragraph::new(message.as_str())
                .block(block)
                .style(Style::default().fg(Color::Red));
            frame.render_widget(paragraph, area);
            return;
        }
    };
    let gutter_width = text::gutter_width(doc.line_count());
    // ハイライトはここで初めて走る。組み立てるのは画面に映る 1 枚分だけ (viewer::render)
    let (rows, first) = viewer
        .render
        .rows(&viewer.highlighter, doc.source(), &viewer.viewport);
    let pane = TextPane {
        window: LineWindow { rows, first },
        changed_lines: &open.changed_lines,
        search: viewer.search.as_ref(),
        selection: viewer.selection.as_ref(),
        cursor: None,
        cursor_band: None,
        gutter_width,
    };
    let visible = pane.visible(&viewer.viewport);
    let paragraph = Paragraph::new(visible)
        .block(block)
        .style(Style::default().bg(background));
    frame.render_widget(paragraph, area);
}
