//! LOG の複数ファイル diff (`git show`) と GIT レーンの「全ファイルまとめ diff」(#31) が
//! 共有するファイル境界の描画ヘルパー。#40 で LOG 用に作った sticky header の見た目を
//! そのまま流用する (境界の色・truncate ロジックを 2 箇所に複製しない)。

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::text;

// sticky header・全幅バンド化した通常のファイル境界行の固定色。端末テーマに依存させないのは
// word-level ハイライトの ADDED_WORD_BG 等と同じ方針
const BOUNDARY_BG: Color = Color::Cyan;
const BOUNDARY_FG: Color = Color::Black;

// sticky 行 (常にペイン上端に固定するファイル名バー) を組み立てる。gutter は持たせず
// (diff 本文ではなくメタ情報のため) 全幅を同じ背景色で埋めて「本文ではない」ことを示す
pub(crate) fn sticky_line(label: &str, width: usize) -> Line<'static> {
    let style = Style::default()
        .fg(BOUNDARY_FG)
        .bg(BOUNDARY_BG)
        .add_modifier(Modifier::BOLD);
    let label = truncate_label(label, width.max(1));
    // 詰める量は char 数ではなくセル幅で測る (widen_row_bands と同じ理由)。
    // 全角を 1 桁と数えると帯がペイン幅を超えて罫線を押し出す
    let pad = width.saturating_sub(text::cells(&label));
    Span::styled(format!("{label}{}", " ".repeat(pad)), style).into()
}

// 流れる側 (スクロールで消えていく通常のファイル境界行) も見た目を強化する。
// render_commit がヘッダ行に付けた固定背景色を目印に、右側をペイン幅まで同じ背景で
// 埋めて全幅の帯にする。gitlane 側の行組み立てには触れず、描画側だけの加工に留める
pub(crate) fn widen_boundary_bands(rows: &mut [Line<'_>], width: usize) {
    for row in rows.iter_mut() {
        let Some(style) = row
            .spans
            .iter()
            .find(|s| s.style.bg == Some(BOUNDARY_BG))
            .map(|s| s.style)
        else {
            continue;
        };
        // 使用済み幅も char 数ではなくセル幅で測る (text_pane::widen_row_bands と同じ)。
        // Line::width は span ごとに text::cells と同じ測り方をするので描画と一致する
        let used = row.width();
        if used < width {
            row.spans
                .push(Span::styled(" ".repeat(width - used), style));
        }
    }
}

// 長いパスは先頭を省略する。末尾のファイル名が最も情報量が多いため、区切り文字境界で
// 前方のディレクトリ階層から落としていき、それでも収まらなければファイル名自体を
// 末尾優先で切る。幅の判定は全て**セル幅**で、char 数では測らない
// (日本語のパスで 1 文字を 1 桁と数えると切り足りず、帯がペイン幅を超える)
fn truncate_label(label: &str, max_width: usize) -> String {
    if text::cells(label) <= max_width {
        return label.to_string();
    }
    let mut parts: Vec<&str> = label.split('/').collect();
    while parts.len() > 1 {
        parts.remove(0);
        let candidate = format!("…/{}", parts.join("/"));
        if text::cells(&candidate) <= max_width {
            return candidate;
        }
    }
    let ellipsis = text::cells("…");
    // 省略記号すら入らない幅では記号を諦めて末尾だけ出す (帯が幅を超えない方を優先する)
    if max_width <= ellipsis {
        return tail_by_cells(label, max_width);
    }
    format!("…{}", tail_by_cells(label, max_width - ellipsis))
}

// 末尾から budget セルぶんを取る。char ではなく grapheme 単位で数えるのは、全角 (1 char =
// 2 セル) と ZWJ 絵文字 (char 数の合計と描画幅が食い違う) のどちらでも桁をずらさないため。
// grapheme 分割は描画とまったく同じ計算になるよう ratatui を通す (text::WrapCursor と同じ)
fn tail_by_cells(label: &str, budget: usize) -> String {
    let span = Span::raw(label);
    let graphemes: Vec<&str> = span
        .styled_graphemes(Style::default())
        .map(|g| g.symbol)
        .collect();
    let mut used = 0usize;
    let mut start = graphemes.len();
    for (i, grapheme) in graphemes.iter().enumerate().rev() {
        let cells = text::cells(grapheme);
        if used + cells > budget {
            break;
        }
        used += cells;
        start = i;
    }
    graphemes[start..].concat()
}

#[cfg(test)]
mod tests {
    use super::{BOUNDARY_BG, sticky_line, truncate_label, widen_boundary_bands};
    use crate::text;
    use ratatui::style::Style;
    use ratatui::text::{Line, Span};

    // 帯は常にペイン幅ちょうどで、超えても足りなくてもいけない (超えると罫線が押し出される)
    #[test]
    fn a_sticky_bar_is_exactly_the_pane_width() {
        for label in ["src/main.rs", "ソース/日本語のパス.rs", "👩\u{200d}💻/a.rs"] {
            for width in [12usize, 20, 40] {
                assert_eq!(
                    sticky_line(label, width).width(),
                    width,
                    "label={label} width={width}"
                );
            }
        }
    }

    // 全角のパスを char 数で数えると切り足りず、帯が幅を超える
    #[test]
    fn truncating_measures_cells_not_chars() {
        let label = "あいうえお/かきくけこ.rs";
        let cut = truncate_label(label, 12);
        assert!(
            text::cells(&cut) <= 12,
            "{cut:?} は 12 セルに収まっていない ({} セル)",
            text::cells(&cut)
        );
        // 末尾のファイル名側を優先して残す
        assert!(cut.ends_with(".rs"), "{cut:?}");
    }

    #[test]
    fn widening_a_boundary_band_measures_cells() {
        let mut rows = vec![Line::from(vec![Span::styled(
            "── 日本語.rs ",
            Style::default().bg(BOUNDARY_BG),
        )])];
        widen_boundary_bands(&mut rows, 40);
        assert_eq!(rows[0].width(), 40);
    }
}
