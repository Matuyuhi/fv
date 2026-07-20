//! 閲覧 (viewer) と編集 (editor) が共有する桁計算の唯一の定義。
//! タブ幅・gutter 幅の解釈が場所によってズレると、検索ハイライト・カーソル・
//! クリック座標の桁対応 (CLAUDE.md の整合インバリアント) が全て壊れるため一箇所に集める。

/// タブ 1 文字の展開結果。normalize と display_col/char_col_at の換算は必ずこれ経由で揃える
pub const TAB_EXPANDED: &str = "    ";

/// 改行を落とし、端末で幅が不定になるタブをスペースに展開する
pub fn normalize(segment: &str) -> String {
    segment
        .trim_end_matches(['\n', '\r'])
        .replace('\t', TAB_EXPANDED)
}

/// 行番号 gutter の全体 char 幅 (行番号の桁数 + 末尾の区切り空白 1 文字)
pub fn gutter_width(line_count: usize) -> usize {
    line_count.max(1).to_string().len() + 1
}

/// バッファ char 座標 → 表示桁 (タブ = TAB_EXPANDED 幅)
pub fn display_col(line: &str, char_col: usize) -> usize {
    line.chars()
        .take(char_col)
        .map(|c| if c == '\t' { TAB_EXPANDED.len() } else { 1 })
        .sum()
}

/// 表示桁 → バッファ char 座標。タブの展開幅の途中はそのタブ自身に丸める
pub fn char_col_at(line: &str, display: usize) -> usize {
    let mut acc = 0;
    for (i, c) in line.chars().enumerate() {
        if acc >= display {
            return i;
        }
        acc += if c == '\t' { TAB_EXPANDED.len() } else { 1 };
        if acc > display {
            return i;
        }
    }
    line.chars().count()
}

/// 論理行が占める視覚行数 (wrap 時)。空行も 1 行を占める
pub fn wrap_rows(display_len: usize, width: usize) -> usize {
    display_len.div_ceil(width.max(1)).max(1)
}
