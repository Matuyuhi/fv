use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};

use crate::app::{App, Mode};
use crate::finder::Finder;

use super::centered_rect;

pub(super) fn draw_finder(frame: &mut Frame, app: &mut App, area: Rect) {
    let Mode::Finder(finder) = &mut app.mode else {
        return;
    };
    let popup = centered_rect(60, 60, area);
    // 下のツリー/ビューアを隠すため、描画前に領域をクリアする
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title("finder (Ctrl+p)");
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let [input_area, list_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
    draw_finder_input(frame, finder, input_area);
    draw_finder_list(frame, finder, list_area);
}

fn draw_finder_input(frame: &mut Frame, finder: &Finder, area: Rect) {
    let query_text = format!("> {}", finder.query);
    let count = format!("{}/{}", finder.matches.len(), finder.total());
    // 入力とカーソルの右側を件数表示までスペースで埋める。入力欄がそれより
    // 狭ければ埋めず単純に連結するだけにする (折り返しは Paragraph に任せる)
    let used = query_text.chars().count() + 1 + count.chars().count();
    let pad = (area.width as usize).saturating_sub(used);
    let line = Line::from(vec![
        Span::raw(query_text),
        // 常に末尾に立つ簡易カーソル (他の入力行と同じ表現)
        Span::styled(" ", Style::default().add_modifier(Modifier::REVERSED)),
        Span::raw(" ".repeat(pad)),
        Span::styled(count, Style::default().fg(Color::DarkGray)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_finder_list(frame: &mut Frame, finder: &mut Finder, area: Rect) {
    let items: Vec<ListItem> = finder
        .matches
        .iter()
        .map(|m| {
            let path = finder.candidate_path(m.candidate).unwrap_or_default();
            ListItem::new(Line::from(highlight_finder_match(path, &m.positions)))
        })
        .collect();
    let list = List::new(items).highlight_style(
        Style::default()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );
    let selected = (!finder.matches.is_empty()).then_some(finder.selected);
    finder.list_state.select(selected);
    frame.render_stateful_widget(list, area, &mut finder.list_state);
}

// マッチした char (positions は昇順) を強調色で塗った span 列を組み立てる
fn highlight_finder_match(path: &str, positions: &[usize]) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut pos_iter = positions.iter().peekable();
    for (i, ch) in path.chars().enumerate() {
        let style = if pos_iter.peek() == Some(&&i) {
            pos_iter.next();
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        spans.push(Span::styled(ch.to_string(), style));
    }
    spans
}
