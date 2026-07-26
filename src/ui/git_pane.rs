use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::Paragraph;

use crate::gitview::GitState;

use super::pane_block;
use super::text_pane::TextPane;

// GitState は App の中にあるので、&App と同時には借りられない。
// 必要な値 (フォーカス・背景色) だけ呼び出し側で取り出して渡す
pub(super) fn draw_git(
    frame: &mut Frame,
    git: &mut GitState,
    focused: bool,
    background: Color,
    area: Rect,
) {
    // キー・マウス処理が次のフレームで読む実測値の書き戻し (viewer_pane と同じパターン)
    git.viewport.height = area.height.saturating_sub(2) as usize;
    git.viewport.width = area.width.saturating_sub(2) as usize;

    let Some(title) = git.title() else {
        let paragraph = Paragraph::new("no file selected")
            .block(pane_block(String::from("diff"), focused))
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(paragraph, area);
        return;
    };
    // hscroll > 0 の間はタイトル側に現在オフセットを出す (viewer と同じ扱い)
    let title = if !git.viewport.wrap && git.viewport.hscroll > 0 {
        format!("{title}  →{}", git.viewport.hscroll)
    } else {
        title.to_string()
    };
    let block = pane_block(title, focused);

    if git.line_count() == 0 {
        let paragraph = Paragraph::new("no changes")
            .block(block)
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(paragraph, area);
        return;
    }
    let pane = TextPane {
        lines: git.lines(),
        // diff 自体が変更の表示なので、閲覧側の変更行マーク・検索・カーソルは全て使わない
        changed_lines: &None,
        search: None,
        cursor: None,
        gutter_width: git.gutter_width(),
    };
    let visible = pane.visible(&git.viewport);
    let paragraph = Paragraph::new(visible)
        .block(block)
        .style(Style::default().bg(background));
    frame.render_widget(paragraph, area);
}
