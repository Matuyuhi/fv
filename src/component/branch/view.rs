use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, HighlightSpacing, List, ListItem, Paragraph};

use crate::component::branch::{BranchRow, BranchState};

use crate::widget::{center_text, centered_rect};

pub(crate) fn draw_branch(frame: &mut Frame, state: &mut BranchState, area: Rect) {
    let popup = centered_rect(70, 60, area);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title("branch (b)");
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let [input_area, list_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
    draw_input(frame, state, input_area);
    draw_list(frame, state, list_area);
}

fn draw_input(frame: &mut Frame, state: &BranchState, area: Rect) {
    let mut spans = vec![Span::raw("> ")];
    let mut used = 2 + 1; // "> " + cursor

    if state.query.is_empty() {
        spans.push(Span::styled(
            " ",
            Style::default().add_modifier(Modifier::REVERSED),
        ));
        let placeholder = "Filter branches...";
        spans.push(Span::styled(
            placeholder,
            Style::default().fg(Color::DarkGray),
        ));
        used += placeholder.chars().count();
    } else {
        spans.push(Span::raw(&state.query));
        spans.push(Span::styled(
            " ",
            Style::default().add_modifier(Modifier::REVERSED),
        ));
        used += state.query.chars().count();
    }

    let count = format!("{}/{}", state.matches.len(), state.total());
    used += count.chars().count();

    let pad = (area.width as usize).saturating_sub(used);
    spans.push(Span::raw(" ".repeat(pad)));
    spans.push(Span::styled(count, Style::default().fg(Color::DarkGray)));

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_list(frame: &mut Frame, state: &mut BranchState, area: Rect) {
    if state.matches.is_empty() {
        let p = Paragraph::new(center_text("no matches", area.height))
            .alignment(ratatui::layout::Alignment::Center)
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(p, area);
        return;
    }

    let items: Vec<ListItem> = state
        .matches
        .iter()
        .map(|m| {
            let spans = state
                .row(m.row)
                .map(|row| branch_line(row, &m.positions))
                .unwrap_or_default();
            ListItem::new(Line::from(spans))
        })
        .collect();
    let list = List::new(items)
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▎")
        .highlight_spacing(HighlightSpacing::Always);
    let selected = (!state.matches.is_empty()).then_some(state.selected);
    state.list_state.select(selected);
    frame.render_stateful_widget(list, area, &mut state.list_state);
}

// 1行 = マーカー(現在ブランチ) + local/remote タグ + ブランチ名 (マッチ char をハイライト) +
// upstream + 相対日時 + 件名。名前列だけ char 単位で分割するのは finder_panel の
// highlight_finder_match と同じ理由 (positions が char インデックスのため)
const NAME_COL_WIDTH: usize = 24;

fn branch_line(row: &BranchRow, positions: &[usize]) -> Vec<Span<'static>> {
    let marker = if row.current { "* " } else { "  " };
    let tag = if row.entry.remote { "remote" } else { "local " };
    let tag_style = if row.entry.remote {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default().fg(Color::Green)
    };
    let upstream = row.entry.upstream.as_deref().unwrap_or("-");

    let mut spans = vec![
        Span::raw(marker.to_string()),
        Span::styled(format!("{tag} "), tag_style),
    ];
    spans.extend(highlight_name(row, positions));
    let name_len = row.entry.name.chars().count();
    spans.push(Span::raw(
        " ".repeat(NAME_COL_WIDTH.saturating_sub(name_len)),
    ));
    spans.push(Span::styled(
        format!(" {upstream:<20}"),
        Style::default().fg(Color::DarkGray),
    ));
    spans.push(Span::styled(
        format!(" {:<12}", row.entry.relative_time),
        Style::default().fg(Color::DarkGray),
    ));
    spans.push(Span::raw(format!(" {}", row.entry.subject)));
    spans
}

fn highlight_name(row: &BranchRow, positions: &[usize]) -> Vec<Span<'static>> {
    let base_style = if row.current {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else if row.entry.remote {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default()
    };
    let match_style = base_style
        .fg(Color::Cyan)
        .add_modifier(Modifier::UNDERLINED);
    let mut spans = Vec::new();
    let mut pos_iter = positions.iter().peekable();
    for (i, ch) in row.entry.name.chars().enumerate() {
        let style = if pos_iter.peek() == Some(&&i) {
            pos_iter.next();
            match_style
        } else {
            base_style
        };
        spans.push(Span::styled(ch.to_string(), style));
    }
    spans
}
