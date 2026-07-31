use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::app::{App, Mode};

// コミットメッセージ入力オーバーレイ。複数行なので Mode::Input (1 行) とは別の描画パスにする。
// カーソルは REVERSED スタイルの重ね書きで表現する (editor と同じ発想: 全角文字幅の計算を
// 避けるため端末カーソルを使わない)。Paragraph::wrap は TextPane では使わない方針だが、ここは
// カーソルが文字に貼り付いたスタイルとして流れるだけなので、外部からの桁計算が要らず問題ない
pub(super) fn draw_commit(frame: &mut Frame, app: &App, area: Rect) {
    let Mode::Commit {
        buffer,
        cursor,
        amend,
        error,
    } = &app.mode
    else {
        return;
    };

    let popup = super::centered_rect(70, 60, area);
    frame.render_widget(Clear, popup);

    let title = if *amend { "amend commit" } else { "commit" };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .title(title);
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let mut constraints = vec![
        Constraint::Length(1), // ルーラー (50/72 桁の目安)
        Constraint::Min(1),    // 本文
        Constraint::Length(1), // キーヒント
    ];
    if error.is_some() {
        constraints.push(Constraint::Length(1));
    }
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    frame.render_widget(Paragraph::new(ruler_line(inner.width as usize)), chunks[0]);
    frame.render_widget(
        Paragraph::new(text_lines(buffer, *cursor)).wrap(Wrap { trim: false }),
        chunks[1],
    );
    frame.render_widget(
        Paragraph::new("Enter: 改行  Ctrl+s: 確定  Esc: 閉じる (下書きを保持)")
            .style(Style::default().fg(Color::DarkGray)),
        chunks[2],
    );
    if let Some(err) = error {
        frame.render_widget(
            Paragraph::new(Span::styled(err.clone(), Style::default().fg(Color::Red))),
            chunks[3],
        );
    }
}

// 1 行目 50 桁・本文 72 桁の目安を区切り線程度で示すだけに留める (issue の要求: 強制はしない)
fn ruler_line(width: usize) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::with_capacity(width);
    for col in 0..width {
        let (ch, color) = if col == 49 || col == 71 {
            ('│', Color::Yellow)
        } else {
            ('·', Color::DarkGray)
        };
        spans.push(Span::styled(ch.to_string(), Style::default().fg(color)));
    }
    spans.push(Span::styled(
        "  50/72 桁の目安",
        Style::default().fg(Color::DarkGray),
    ));
    Line::from(spans)
}

fn text_lines(buffer: &str, cursor: usize) -> Vec<Line<'static>> {
    let (cursor_line, cursor_col) = line_col(buffer, cursor);
    buffer
        .split('\n')
        .enumerate()
        .map(|(i, line)| {
            if i != cursor_line {
                return Line::from(line.to_string());
            }
            let chars: Vec<char> = line.chars().collect();
            let mut spans = Vec::new();
            if cursor_col > 0 {
                spans.push(Span::raw(chars[..cursor_col].iter().collect::<String>()));
            }
            if cursor_col < chars.len() {
                spans.push(Span::styled(
                    chars[cursor_col].to_string(),
                    Style::default().add_modifier(Modifier::REVERSED),
                ));
                if cursor_col + 1 < chars.len() {
                    spans.push(Span::raw(
                        chars[cursor_col + 1..].iter().collect::<String>(),
                    ));
                }
            } else {
                spans.push(Span::styled(
                    " ",
                    Style::default().add_modifier(Modifier::REVERSED),
                ));
            }
            Line::from(spans)
        })
        .collect()
}

fn line_col(buffer: &str, cursor: usize) -> (usize, usize) {
    let mut line = 0usize;
    let mut col = 0usize;
    for (i, ch) in buffer.chars().enumerate() {
        if i == cursor {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    (line, col)
}
