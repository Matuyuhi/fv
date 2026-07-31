use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{List, ListItem, Paragraph};

use crate::logview::LogState;

use super::pane_block;
use super::text_pane::TextPane;

// 左ペイン: コミット一覧。ツリーではなく LogState が持つ commits を直接描く
pub(super) fn draw_log_list(frame: &mut Frame, log: &mut LogState, focused: bool, area: Rect) {
    let title = format!("log ({})", log.commits().len());
    if log.commits().is_empty() {
        let paragraph = Paragraph::new("no commits")
            .block(pane_block(title, focused))
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(paragraph, area);
        return;
    }
    let open_index = log.open_index();
    let items: Vec<ListItem> = log
        .commits()
        .iter()
        .enumerate()
        .map(|(i, commit)| {
            // diff を開いている行だけ印を付ける (selected と別概念: j/k では動かない)
            let marker = if Some(i) == open_index { "▶ " } else { "  " };
            let label = format!(
                "{marker}{}  {:<15}  {:<12}  {}",
                commit.short, commit.relative_time, commit.author, commit.subject
            );
            ListItem::new(label)
        })
        .collect();
    let list = List::new(items)
        .block(pane_block(title, focused))
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );
    frame.render_stateful_widget(list, area, &mut log.list_state);
}

// 右ペイン: 選択コミットの diff。GIT レーンの diff ペインと基本構造は同じだが、
// 基準 (base) の概念が無いのでタイトルにコミット情報を出すだけで良い
pub(super) fn draw_log_diff(
    frame: &mut Frame,
    log: &mut LogState,
    focused: bool,
    background: Color,
    area: Rect,
) {
    log.viewport.height = area.height.saturating_sub(2) as usize;
    log.viewport.width = area.width.saturating_sub(2) as usize;

    let Some(title) = log.title() else {
        let paragraph = Paragraph::new("Enter/l: open diff")
            .block(pane_block("diff".to_string(), focused))
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(paragraph, area);
        return;
    };
    let title = if !log.viewport.wrap && log.viewport.hscroll > 0 {
        format!("{title}  →{}", log.viewport.hscroll)
    } else {
        title
    };
    let block = pane_block(title, focused);

    if log.line_count() == 0 {
        let paragraph = Paragraph::new("no changes")
            .block(block)
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(paragraph, area);
        return;
    }
    let pane = TextPane {
        lines: log.lines(),
        changed_lines: &None,
        search: None,
        cursor: None,
        gutter_width: log.gutter_width(),
    };
    let visible = pane.visible(&log.viewport);
    let paragraph = Paragraph::new(visible)
        .block(block)
        .style(Style::default().bg(background));
    frame.render_widget(paragraph, area);
}
