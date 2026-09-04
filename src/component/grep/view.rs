use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};

use super::{GrepState, search};
use crate::widget::{centered_rect, visible_window};

pub(crate) fn draw_grep(frame: &mut Frame, grep: &mut GrepState, area: Rect) {
    let popup = centered_rect(80, 80, area);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(title(grep));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let [input_area, list_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
    draw_input(frame, grep, input_area);
    draw_list(frame, grep, list_area);
}

// 走査の状態をタイトルに出す。「終わりなのか途中なのか」「全部なのか打ち切りなのか」が
// 一覧の見た目からは区別できないため、必ずここで言葉にする
fn title(grep: &GrepState) -> String {
    let mut title = String::from("grep (Ctrl+f)");
    if !grep.searchable() {
        return title;
    }
    title.push_str(&format!(
        "  {} hits in {} files",
        grep.hit_count(),
        grep.file_count()
    ));
    if grep.busy() {
        title.push_str("  searching...");
    } else {
        title.push_str(&format!("  ({} files scanned)", grep.scanned()));
    }
    if grep.truncated() {
        title.push_str(&format!("  truncated at {}", search::MAX_HITS));
    }
    if grep.stale() {
        title.push_str("  stale");
    }
    title
}

fn draw_input(frame: &mut Frame, grep: &GrepState, area: Rect) {
    let line = Line::from(vec![
        Span::raw(format!("> {}", grep.query)),
        // 常に末尾に立つ簡易カーソル (Finder と同じ表現)
        Span::styled(" ", Style::default().add_modifier(Modifier::REVERSED)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_list(frame: &mut Frame, grep: &mut GrepState, area: Rect) {
    let total = grep.rows().len();
    if total == 0 || area.height == 0 {
        let message = if !grep.searchable() {
            "type 2+ characters to search the workspace"
        } else if grep.busy() {
            "searching..."
        } else {
            "no matches"
        };
        frame.render_widget(
            Paragraph::new(Span::styled(message, Style::default().fg(Color::DarkGray))),
            area,
        );
        return;
    }
    let selected = Some(grep.selected);
    // ヒットは最大 MAX_HITS 件並ぶので、ツリーと同じく画面に映る範囲だけ ListItem を組む
    let (first, last) = visible_window(
        total,
        area.height as usize,
        *grep.list_state.offset_mut(),
        selected,
    );
    *grep.list_state.offset_mut() = first;

    let items: Vec<ListItem> = grep.rows()[first..last]
        .iter()
        .map(|row| {
            let file = &grep.files()[row.file];
            let hit = &file.hits[row.hit];
            ListItem::new(Line::from(hit_spans(&file.path.to_string_lossy(), hit)))
        })
        .collect();
    let list = List::new(items).highlight_style(
        Style::default()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );
    // List には切り出した部分列を渡すので、選択位置も offset もそれに合わせて相対化する
    // (絶対値の list_state をそのまま渡すと二重にずれる。tree/view.rs と同じ)
    let mut window_state = ListState::default().with_selected(selected.map(|s| s - first));
    frame.render_stateful_widget(list, area, &mut window_state);
}

// `path:line: ` を暗く、本文のマッチ部分だけを強調する
fn hit_spans(path: &str, hit: &search::Hit) -> Vec<Span<'static>> {
    let mut spans = vec![
        Span::styled(path.to_string(), Style::default().fg(Color::Cyan)),
        Span::styled(
            format!(":{}: ", hit.line + 1),
            Style::default().fg(Color::DarkGray),
        ),
    ];
    if hit.clipped {
        spans.push(Span::styled("…", Style::default().fg(Color::DarkGray)));
    }
    let chars: Vec<char> = hit.text.chars().collect();
    let start = hit.start_col.min(chars.len());
    let end = hit.end_col.min(chars.len());
    let before: String = chars[..start].iter().collect();
    let matched: String = chars[start..end].iter().collect();
    let after: String = chars[end..].iter().collect();
    spans.push(Span::raw(before));
    spans.push(Span::styled(
        matched,
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::raw(after));
    spans
}
