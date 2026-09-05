use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::{App, CommitField, Mode};
use crate::lang::{Msg, t};

// コミットメッセージ入力オーバーレイ。件名 (1 行) と本文 (複数行) を別の欄として描く。
// カーソルは REVERSED スタイルの重ね書きで表現する (editor と同じ発想: 全角文字幅の計算を
// 避けるため端末カーソルを使わない)。折返しは TextPane と同じく char 単位の自前分割にする —
// Paragraph::wrap (WordWrapper) は中身が空白だけの行を 2 行に割るため、空のメッセージだと
// カーソル (行末の REVERSED 空白) が 1 行下へずれてしまう
pub(super) fn draw_commit(frame: &mut Frame, app: &App, area: Rect) {
    let Mode::Commit {
        draft,
        amend,
        error,
    } = &app.mode
    else {
        return;
    };

    let popup = crate::widget::centered_rect(70, 60, area);
    frame.render_widget(Clear, popup);

    let title = if *amend { "amend commit" } else { "commit" };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .title(title);
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let width = inner.width as usize;
    let on_subject = draft.field == CommitField::Subject;
    // 件名欄の高さは折返し後の行数ぶんだけ確保する (通常は 1 行、長い件名でも溢れない)
    let subject_lines = text_lines(
        &draft.subject,
        on_subject.then_some(draft.subject_cursor),
        width,
    );
    let body_lines = text_lines(
        &draft.body,
        (!on_subject).then_some(draft.body_cursor),
        width,
    );

    let mut constraints = vec![
        Constraint::Length(subject_lines.len() as u16), // 件名
        Constraint::Length(1),                          // 区切り (件名の文字数を右端に置く)
        Constraint::Min(1),                             // 本文
        Constraint::Length(1),                          // キーヒント
    ];
    if error.is_some() {
        constraints.push(Constraint::Length(1));
    }
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    frame.render_widget(Paragraph::new(subject_lines), chunks[0]);
    frame.render_widget(
        Paragraph::new(divider_line(draft.subject.chars().count(), width)),
        chunks[1],
    );
    frame.render_widget(Paragraph::new(body_lines), chunks[2]);
    let hint = if on_subject {
        t(Msg::CommitTabEnterBodyCtrlCmd)
    } else {
        t(Msg::CommitTabSubjectEnterNewlineCtrl)
    };
    frame.render_widget(
        Paragraph::new(hint).style(Style::default().fg(Color::DarkGray)),
        chunks[3],
    );
    if let Some(err) = error {
        frame.render_widget(
            Paragraph::new(Span::styled(err.clone(), Style::default().fg(Color::Red))),
            chunks[4],
        );
    }
}

// 件名の推奨上限 (git の慣習)。超えても入力は妨げず、カウンタの色を変えて知らせるだけ
const SUBJECT_LIMIT: usize = 50;

// 件名と本文の区切り。右端に件名の文字数を出して、50 桁の目安を数字で分かるようにする
fn divider_line(len: usize, width: usize) -> Line<'static> {
    let label = format!(" {len}/{SUBJECT_LIMIT} ");
    let rule = width.saturating_sub(label.chars().count() + 1);
    let color = if len > SUBJECT_LIMIT {
        Color::Yellow
    } else {
        Color::DarkGray
    };
    Line::from(vec![
        Span::styled("─".repeat(rule), Style::default().fg(Color::DarkGray)),
        Span::styled(label, Style::default().fg(color)),
        Span::styled("─", Style::default().fg(Color::DarkGray)),
    ])
}

// cursor が None の欄 (フォーカスしていない側) はカーソルを重ねずそのまま描く
fn text_lines(buffer: &str, cursor: Option<usize>, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let (cursor_line, cursor_col) = match cursor {
        Some(cursor) => {
            let (line, col) = line_col(buffer, cursor);
            (Some(line), col)
        }
        None => (None, 0),
    };
    let mut out = Vec::new();
    for (i, line) in buffer.split('\n').enumerate() {
        let mut chars: Vec<char> = line.chars().collect();
        let cursor_col = (Some(i) == cursor_line).then_some(cursor_col);
        // カーソルが行末にあるときは重ねる相手が無いので空白を 1 つ足して同じ扱いにする
        if cursor_col.is_some_and(|col| col >= chars.len()) {
            chars.push(' ');
        }
        if chars.is_empty() {
            out.push(Line::from(String::new()));
            continue;
        }
        for (row, chunk) in chars.chunks(width).enumerate() {
            let base = row * width;
            let mut spans = Vec::new();
            match cursor_col.filter(|col| (base..base + chunk.len()).contains(col)) {
                Some(col) => {
                    let at = col - base;
                    if at > 0 {
                        spans.push(Span::raw(chunk[..at].iter().collect::<String>()));
                    }
                    spans.push(Span::styled(
                        chunk[at].to_string(),
                        Style::default().add_modifier(Modifier::REVERSED),
                    ));
                    if at + 1 < chunk.len() {
                        spans.push(Span::raw(chunk[at + 1..].iter().collect::<String>()));
                    }
                }
                None => spans.push(Span::raw(chunk.iter().collect::<String>())),
            }
            out.push(Line::from(spans));
        }
    }
    out
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
