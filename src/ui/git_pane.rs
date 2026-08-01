use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::gitview::{self, GitState};

use super::diff_boundary::{sticky_line, widen_boundary_bands};
use super::pane_block;
use super::text_pane::{LineWindow, TextPane};

// GitState は App の中にあるので、&App と同時には借りられない。
// 必要な値 (フォーカス・背景色) だけ呼び出し側で取り出して渡す
pub(super) fn draw_git(
    frame: &mut Frame,
    git: &mut GitState,
    focused: bool,
    background: Color,
    area: Rect,
) {
    let inner_width = area.width.saturating_sub(2) as usize;
    // まとめ diff (#31) の sticky header に 1 行使う分だけ TextPane へ渡す高さを削る。
    // LOG レーンの draw_log_diff と同じく scroll ではなく「境界を持つか」だけで決める
    // (scroll 依存にすると Ctrl+d/Ctrl+u のページ送り量がスクロール中に変わってしまう)
    let sticky_reserved = usize::from(git.has_file_boundary());
    // キー・マウス処理が次のフレームで読む実測値の書き戻し (viewer_pane と同じパターン)。
    // side-by-side のカラム幅もここに書いた width から導出する (GitState::column_width)
    git.viewport.height = (area.height.saturating_sub(2) as usize).saturating_sub(sticky_reserved);
    git.viewport.width = inner_width;

    let Some(title) = git.title() else {
        let title = format!("diff [{}]", git.base_label());
        let paragraph = Paragraph::new("no file selected")
            .block(pane_block(title, focused))
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(paragraph, area);
        return;
    };
    // 現在の diff 基準を常にタイトルに出す。hscroll > 0 の間はさらにオフセットも足す
    // (viewer の hscroll 表示と同じ場所・作法)。side-by-side を要求していても幅不足で
    // inline に落ちている間は、それが分かるようヒントを足す
    let mut title = format!("{title}  [{}]", git.base_label());
    if git.side_by_side_requested() && !git.side_by_side_active() {
        title.push_str("  (narrow: inline)");
    } else if git.side_by_side_active() {
        title.push_str("  [side-by-side]");
    }
    let title = if !git.viewport.wrap && git.viewport.hscroll > 0 {
        format!("{title}  →{}", git.viewport.hscroll)
    } else {
        title
    };

    if git.line_count() == 0 {
        let paragraph = Paragraph::new("no changes")
            .block(pane_block(title, focused))
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(paragraph, area);
        return;
    }

    if git.side_by_side_active() {
        draw_side_by_side(frame, git, focused, background, area, title);
        return;
    }

    let pane = TextPane {
        window: LineWindow::slice(git.lines(), &git.viewport),
        // diff 自体が変更の表示なので、閲覧側の変更行マークは使わない。カーソルも同様。
        // 検索 (#31) は inline 表示 (単一ファイル/まとめ diff とも) でだけ有効にする
        changed_lines: &None,
        search: git.search(),
        cursor: None,
        gutter_width: git.gutter_width(),
    };
    let mut visible = pane.visible(&git.viewport);
    widen_boundary_bands(&mut visible, inner_width);
    if let Some(label) = git.sticky_label() {
        visible.insert(0, sticky_line(label, inner_width));
    }
    let paragraph = Paragraph::new(visible)
        .block(pane_block(title, focused))
        .style(Style::default().bg(background));
    frame.render_widget(paragraph, area);
}

// side-by-side (左 = 旧, 右 = 新) 描画。外枠は 1 つで内側を左右 2 分割し、間に 1 桁の
// 区切り罫線を挟む。折返し中は gitview::side_by_side_wrapped で char 単位に事前分割・
// 行数を揃えた列を都度作り直す (wrap 幅は実測でしか出せないため、ここは他の描画パイプ
// ラインと同じく毎フレーム計算する)。事前に行数を揃えてあるぶん TextPane 自体は非 wrap
// のまま普通にスライスするだけで済み、text_pane.rs に side-by-side 専用の分岐は増えない
fn draw_side_by_side(
    frame: &mut Frame,
    git: &mut GitState,
    focused: bool,
    background: Color,
    area: Rect,
    title: String,
) {
    let block = pane_block(title, focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let column_width = git.column_width() as u16;
    let [left_area, sep_area, right_area] = Layout::horizontal([
        Constraint::Length(column_width),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(inner);

    let (left_gutter, right_gutter) = git.side_gutter_widths();
    let (left_src, right_src) = git.side_lines();

    // 折返し ON: 事前分割 + 行数揃えの結果をそのまま使う (非 wrap 前提で TextPane に渡す)。
    // 折返し OFF: side_lines の時点で既に行数が揃っている (render_side_by_side が保証)
    let owned;
    let (left_lines, right_lines): (&[_], &[_]) = if git.viewport.wrap {
        let (l, r, hunks) = gitview::side_by_side_wrapped(
            left_src,
            right_src,
            git.side_hunks(),
            left_gutter,
            right_gutter,
            git.column_width(),
        );
        git.set_side_wrap_cache(l.len(), hunks);
        owned = (l, r);
        (&owned.0, &owned.1)
    } else {
        (left_src, right_src)
    };

    // TextPane の非 wrap パスは vp.scroll を「両カラムで揃えた行 index」としてそのまま
    // スライスに使う。wrap 中でも side-by-side は自前で分割済みなので wrap=false で渡す
    let mut vp = git.viewport.clone();
    vp.wrap = false;

    let left_pane = TextPane {
        window: LineWindow::slice(left_lines, &vp),
        changed_lines: &None,
        search: None,
        cursor: None,
        gutter_width: left_gutter,
    };
    let right_pane = TextPane {
        window: LineWindow::slice(right_lines, &vp),
        changed_lines: &None,
        search: None,
        cursor: None,
        gutter_width: right_gutter,
    };
    let left_visible = left_pane.visible(&vp);
    let right_visible = right_pane.visible(&vp);

    frame.render_widget(
        Paragraph::new(left_visible).style(Style::default().bg(background)),
        left_area,
    );
    frame.render_widget(
        Paragraph::new(right_visible).style(Style::default().bg(background)),
        right_area,
    );
    let separator = Block::default()
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(Color::DarkGray));
    frame.render_widget(separator, sep_area);
}
