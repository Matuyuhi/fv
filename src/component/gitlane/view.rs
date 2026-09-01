use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::component::gitlane::GitState;

use crate::widget::diff_boundary::{sticky_line, widen_boundary_bands};
use crate::widget::pane_block;
use crate::widget::text_pane::{LineWindow, TextPane, widen_row_bands};

// GitState は App の中にあるので、&App と同時には借りられない。
// 必要な値 (フォーカス・背景色) だけ呼び出し側で取り出して渡す
pub(crate) fn draw_git(
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
        let paragraph =
            Paragraph::new("No file selected\n\nSelect a changed file to view its diff")
                .block(pane_block(title, focused))
                .alignment(ratatui::layout::Alignment::Center)
                .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(paragraph, area);
        return;
    };
    // 現在の diff 基準を常にタイトルに出す。hscroll > 0 の間はさらにオフセットも足す
    // (viewer の hscroll 表示と同じ場所・作法)。side-by-side を要求していても幅不足で
    // inline に落ちている間は、それが分かるようヒントを足す
    let mut title = format!("{title}  [{}]", git.base_label());
    // Space の対象 (カーソル行が属する hunk) を暗黙にしないため、序数を常に出す
    if let Some((ordinal, total)) = git.hunk_position() {
        title.push_str(&format!("  hunk {ordinal}/{total}"));
    }
    // 行単位選択中は何行掴んでいるかを出す。帯の色だけだと画面外へ伸びた分が読めない
    if let Some(rows) = git.selected_row_count() {
        title.push_str(&format!("  {rows} lines selected"));
    }
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
        // diff 自体が変更の表示なので、閲覧側の変更行マーク・char 単位カーソルは使わない
        // (行カーソルは focus_row の帯で出す)。検索 (#31) は inline 表示
        // (単一ファイル/まとめ diff とも) でだけ有効にする
        changed_lines: &None,
        search: git.search(),
        selection: None,
        cursor: None,
        focus_row: focused.then(|| git.cursor()),
        selected_rows: focused.then(|| git.line_selection()).flatten(),
        gutter_width: git.gutter_width(),
    };
    let mut visible = pane.visible(&git.viewport);
    widen_row_bands(&mut visible, inner_width);
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
// 区切り罫線を挟む。折返し中は GitState::side_wrapped が char 単位に事前分割・行数を
// 揃えた列を返す (wrap 幅は実測でしか出せないので作るのは描画時だが、幅も diff も
// 変わらなければ作り直さない)。事前に行数を揃えてあるぶん TextPane 自体は非 wrap の
// まま普通にスライスするだけで済み、text_pane.rs に side-by-side 専用の分岐は増えない
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

    // TextPane の非 wrap パスは vp.scroll を「両カラムで揃えた行 index」としてそのまま
    // スライスに使う。wrap 中でも side-by-side は自前で分割済みなので wrap=false で渡す
    let mut vp = git.viewport.clone();
    let wrapped = vp.wrap;
    vp.wrap = false;

    // 左右は同じ行 index で対応が取れているので、帯も同じ行に出せば 1 本に見える。
    // git を読む値はここで全て取り出しておく — 下の side_wrapped が &mut を要求するため
    let focus_row = focused.then(|| git.cursor());
    let selected_rows = focused.then(|| git.line_selection()).flatten();

    // 折返し ON: 事前分割 + 行数揃えの結果をそのまま使う (非 wrap 前提で TextPane に渡す)。
    // 幅も diff も変わっていなければ GitState 側のキャッシュがそのまま返る。
    // 折返し OFF: side_lines の時点で既に行数が揃っている (render_side_by_side が保証)
    let (left_lines, right_lines): (&[_], &[_]) = if wrapped {
        let cache = git.side_wrapped();
        (&cache.left, &cache.right)
    } else {
        git.side_lines()
    };
    let left_pane = TextPane {
        window: LineWindow::slice(left_lines, &vp),
        changed_lines: &None,
        search: None,
        selection: None,
        cursor: None,
        focus_row,
        selected_rows,
        gutter_width: left_gutter,
    };
    let right_pane = TextPane {
        window: LineWindow::slice(right_lines, &vp),
        changed_lines: &None,
        search: None,
        selection: None,
        cursor: None,
        focus_row,
        selected_rows,
        gutter_width: right_gutter,
    };
    let mut left_visible = left_pane.visible(&vp);
    let mut right_visible = right_pane.visible(&vp);
    widen_row_bands(&mut left_visible, left_area.width as usize);
    widen_row_bands(&mut right_visible, right_area.width as usize);

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
