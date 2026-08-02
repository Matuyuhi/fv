use std::path::Path;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState};

use crate::component::tree::Tree;
use crate::git::{FileStatus, GitStatus, StatusKind};

use crate::widget::pane_block;

pub(crate) fn draw_tree(
    frame: &mut Frame,
    tree: &mut Tree,
    git: Option<&GitStatus>,
    root: &Path,
    icons: bool,
    focused: bool,
    area: Rect,
) {
    // GIT レーンでは絞り込み中であることをタイトルで示す (行が減った理由が分かるように)
    let title = if tree.is_filtered() {
        format!("changes ({})", tree.visible_files())
    } else {
        root.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.display().to_string())
    };
    let block = pane_block(title, focused);
    let inner = block.inner(area);

    let total = tree.visible.len();
    let selected = (total > 0).then_some(tree.selected);
    tree.list_state.select(selected);

    // 行の高さは gutter 込みでも常に 1 (name に改行は入らない) なので、ratatui の
    // List が内部でやる「選択行を含む最小限のウィンドウ計算」は同じ結果になる
    // O(1) の式に置き換えられる。ここで [first, last) を確定させ、ListItem は
    // その範囲だけ組み立てる (visible 全体ではなく画面に映る行数に比例させるのが目的)
    let max_height = inner.height as usize;
    let (first, last) = if total == 0 || max_height == 0 {
        (0, 0)
    } else {
        visible_window(total, max_height, *tree.list_state.offset_mut(), selected)
    };
    // list_state.offset() は app/mouse.rs::click_tree_row がクリック行の絶対 index 換算に使う
    // (ui→app の書き戻しパターン、tree_area 等と同じ)。ratatui 標準の List に描画を任せると
    // ウィンドウ切り出し分だけ相対化されてしまうため、絶対値をこちらで書き戻す
    *tree.list_state.offset_mut() = first;

    let items: Vec<ListItem> = tree.visible[first..last]
        .iter()
        .map(|row| {
            // アイコン有効時は folder の開閉アイコンが展開状態を兼ねるためマーカー不要
            let marker = if icons {
                ""
            } else if row.is_dir {
                if row.expanded { "▾ " } else { "▸ " }
            } else {
                "  "
            };
            // ディレクトリは git.files に直接エントリを持たないため自然に None になる
            let file_status = git.and_then(|g| g.files.get(&row.path).copied());
            let prefix = file_status.map(status_prefix).unwrap_or_default();
            let icon = if icons {
                let glyph = if row.is_dir {
                    crate::widget::icons::dir_icon(row.expanded)
                } else {
                    crate::widget::icons::file_icon(&row.name)
                };
                format!("{glyph} ")
            } else {
                String::new()
            };
            // 行頭の XY マーカーは git status 準拠の赤/緑。ファイル名は stage 済みの時だけ
            // 黄色にし、未ステージの間は通常色のまま (常時色を付けると名前が読みにくくなる)
            let mut spans = vec![Span::raw(format!("{}{}", "  ".repeat(row.depth), marker))];
            if let Some(status) = file_status {
                spans.push(Span::styled(
                    prefix,
                    Style::default().fg(status_color(status)),
                ));
            }
            // ディレクトリもファイル名と同じ基準で黄色にする。畳んだままの add でも
            // 色が変わらないと「乗ったのか」が分からないため、配下が全て stage 済みの
            // ディレクトリだけを黄色にする (未ステージが残っていれば通常のディレクトリ色)
            let staged = if row.is_dir {
                git.is_some_and(|g| {
                    g.changed_dirs.contains(&row.path) && !g.unstaged_dirs.contains(&row.path)
                })
            } else {
                file_status.is_some_and(|s| s.worktree.is_none())
            };
            let name_style = if staged {
                Style::default().fg(Color::Yellow)
            } else if row.is_dir {
                Style::default().fg(Color::Blue)
            } else {
                Style::default()
            };
            spans.push(Span::styled(format!("{icon}{}", row.name), name_style));
            ListItem::new(Line::from(spans))
        })
        .collect();
    let list = List::new(items).block(block).highlight_style(
        Style::default()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );
    // items は [first, last) だけの部分列なので、List に渡す選択位置もその中の相対位置に
    // 直す必要がある。絶対値は app.tree.list_state (offset() 経由でクリック判定が読む) 側に
    // 既に書き戻し済みなので、ここは使い捨ての一時 state で構わない
    let mut render_state = ListState::default();
    if let Some(sel) = selected {
        render_state.select(Some(sel - first));
    }
    frame.render_stateful_widget(list, area, &mut render_state);
}

/// ratatui `List` が内部で行う「選択行を含む最小限のウィンドウ」計算 (get_items_bounds) と
/// 等価な結果を返す。ツリーの行は全て高さ1 (name に改行は入らない) なので、あちらのような
/// 可変高さ対応のループは要らず、offset を起点に selected が入るまでスライドするだけの
/// O(1) 計算に落とせる。selected が既にウィンドウ内なら offset をそのまま保つのがポイントで、
/// ここを毎回 selected 中心に作り直すと「選択が動くたびに画面が揺れる」挙動になってしまう
fn visible_window(
    total: usize,
    max_height: usize,
    offset: usize,
    selected: Option<usize>,
) -> (usize, usize) {
    let offset = offset.min(total - 1);
    let index_to_display = selected.map(|s| s.min(total - 1)).unwrap_or(offset);
    let mut first = offset;
    let mut last = (offset + max_height).min(total);
    if index_to_display >= last {
        first = index_to_display + 1 - max_height.min(index_to_display + 1);
        last = (first + max_height).min(total);
    } else if index_to_display < first {
        first = index_to_display;
        last = (first + max_height).min(total);
    }
    (first, last)
}

// ツリーの行頭に置く XY (index 側 + worktree 側) + 空白のマーカー。
// git status --short と同じ並びで "M " = ステージ済みのみ、" M" = 未ステージのみ、
// "MM" = 両方、"??" = untracked を表す
fn status_prefix(status: FileStatus) -> String {
    format!(
        "{}{} ",
        status_char(status.index),
        status_char(status.worktree)
    )
}

fn status_char(kind: Option<StatusKind>) -> char {
    match kind {
        None => ' ',
        Some(StatusKind::Modified) => 'M',
        Some(StatusKind::Added) => 'A',
        Some(StatusKind::Untracked) => '?',
        Some(StatusKind::Deleted) => 'D',
        Some(StatusKind::Renamed) => 'R',
    }
}

// git status のデフォルト配色に揃える: stage 済み (added/updated) が緑、未ステージ
// (changed/untracked) が赤。worktree 側が None = 未ステージの変更が残っていない状態
// だけを緑にするので、"MM" (一部だけ stage) は赤のままになる。両方 None は
// file_status が Some を返す限り実際には起こらない (porcelain の行は必ずどちらかに変更を持つ)
fn status_color(status: FileStatus) -> Color {
    match status.worktree {
        None => Color::Green,
        Some(_) => Color::Red,
    }
}
