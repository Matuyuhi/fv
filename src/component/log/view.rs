use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{List, ListItem, Paragraph};

use crate::component::log::LogState;

use crate::widget::diff_boundary::{sticky_line, widen_boundary_bands};
use crate::widget::pane_block;
use crate::widget::text_pane::{LineWindow, TextPane, widen_row_bands};

// 件名を最優先で残し、狭い幅では右側の列から落とす閾値 (issues/PR の一覧と同じ考え方)。
// ツリーと同じ左ペインに同居するようになり、単独レーンだった頃の幅は前提にできない
const AUTHOR_MIN_WIDTH: usize = 60;
const TIME_MIN_WIDTH: usize = 40;

// コミット一覧。ツリーの下に並ぶ独立ペインで、LogState が持つ commits を直接描く
pub(crate) fn draw_log_list(frame: &mut Frame, log: &mut LogState, focused: bool, area: Rect) {
    let title = format!("log ({})", log.commits().len());
    if log.commits().is_empty() {
        let paragraph = Paragraph::new("no commits")
            .block(pane_block(title, focused))
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(paragraph, area);
        return;
    }
    let inner_width = area.width.saturating_sub(2) as usize;
    let open_index = log.open_index();
    let items: Vec<ListItem> = log
        .commits()
        .iter()
        .enumerate()
        .map(|(i, commit)| {
            // diff を開いている行だけ印を付ける (selected と別概念: j/k では動かない)
            let marker = if Some(i) == open_index { "▶ " } else { "  " };
            let mut label = format!("{marker}{}  ", commit.short);
            if inner_width >= TIME_MIN_WIDTH {
                label.push_str(&format!("{:<15}  ", commit.relative_time));
            }
            if inner_width >= AUTHOR_MIN_WIDTH {
                label.push_str(&format!("{:<12}  ", commit.author));
            }
            label.push_str(&commit.subject);
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
pub(crate) fn draw_log_diff(
    frame: &mut Frame,
    log: &mut LogState,
    focused: bool,
    background: Color,
    area: Rect,
) {
    let inner_width = area.width.saturating_sub(2) as usize;
    // sticky header に 1 行使う分だけ TextPane へ渡す高さを削る。scroll 位置ではなく
    // 「このコミットの diff にファイル境界があるか」だけで決めるのが要点: scroll 依存にすると
    // コミットメッセージ部分とファイル本文とで高さが変わり、Ctrl+d/Ctrl+u のページ送り量が
    // スクロール中に狂う (キー処理側は書き戻し後の viewport.height をそのまま読む)
    let sticky_reserved = usize::from(log.has_file_boundary());
    log.viewport.height = (area.height.saturating_sub(2) as usize).saturating_sub(sticky_reserved);
    log.viewport.width = inner_width;

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
        window: LineWindow::slice(log.lines(), &log.viewport),
        changed_lines: &None,
        search: None,
        selection: None,
        cursor: None,
        // 帯を出すのはこのペインにフォーカスがある間だけ (draw_viewer/draw_git と同じ)
        focus_row: focused.then(|| log.cursor()),
        selected_rows: None,
        gutter_width: log.gutter_width(),
    };
    let mut rows = pane.visible(&log.viewport);
    widen_row_bands(&mut rows, inner_width);
    widen_boundary_bands(&mut rows, inner_width);
    if let Some(label) = log.sticky_label() {
        rows.insert(0, sticky_line(label, inner_width));
    }
    let paragraph = Paragraph::new(rows)
        .block(block)
        .style(Style::default().bg(background));
    frame.render_widget(paragraph, area);
}
