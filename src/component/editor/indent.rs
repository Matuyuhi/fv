//! Enter の自動インデントと、Enter/Tab が使うインデント 1 段の推定。
//! word.rs と同じく状態を持たない純関数に閉じ、EditState 側には「バッファへどう適用するか」
//! だけを残す。
//!
//! 「AI が書いたコードを手直しする」用途では、行を足すたびにインデントを打ち直すのが
//! 一番の手間になる。一方で言語ごとの規則 (Python の `:`・Ruby の `do` 等) を持ち始めると
//! キリが無いので、**前の行のインデントを引き継ぐ**のと**開き括弧の直後で 1 段下げる**の
//! 2 つだけにしてある (どの言語でもまず外れない最小限)。

/// インデント 1 段の推定で見る範囲 (カーソル行の前後それぞれ)。文書全体を舐めると
/// 1 打鍵のコストがファイルの大きさに比例するので、近所だけで決める
const SCAN_LINES: usize = 200;
/// 推定に使える材料が無い時の 1 段
const DEFAULT_UNIT: &str = "    ";

/// Enter 1 回ぶんの編集。行内の `[from, col)` を `text` へ差し替え、カーソルを
/// 「カーソル行から `cursor.0` 行下の `cursor.1` 桁」へ置く
#[derive(Debug, PartialEq, Eq)]
pub(super) struct LineBreak {
    pub from: usize,
    pub text: String,
    pub cursor: (usize, usize),
}

/// col (char インデックス) で Enter を押した時の編集を返す。
/// - 新しい行はカーソル行のインデントを引き継ぐ (カーソルがインデントの途中ならそこまで)
/// - カーソルの手前が開き括弧なら 1 段下げ、直後が対応する閉じ括弧ならそれを
///   元の深さの次の行へ送る (`{|}` → 3 行に開く)
/// - インデントしか無い行で押したら、その行の空白は消す (空行に空白を残さない)
pub(super) fn line_break(line: &str, col: usize, unit: &str) -> LineBreak {
    let chars: Vec<char> = line.chars().collect();
    let col = col.min(chars.len());
    let indent_len = chars.iter().take_while(|c| is_indent(**c)).count();
    let indent: String = chars[..indent_len.min(col)].iter().collect();
    let before = &chars[..col];
    let after = &chars[col..];

    if col <= indent_len && after.iter().all(|c| is_indent(*c)) {
        // 空白だけの行: 空白を新しい行へ持っていき、元の行は空にする
        return LineBreak {
            from: 0,
            text: format!("\n{indent}"),
            cursor: (1, indent.chars().count()),
        };
    }

    let opener = before.iter().rev().find(|c| !is_indent(**c)).copied();
    let Some(closer) = opener.and_then(closing) else {
        return LineBreak {
            from: col,
            text: format!("\n{indent}"),
            cursor: (1, indent.chars().count()),
        };
    };
    let inner = format!("{indent}{unit}");
    let inner_len = inner.chars().count();
    if after.iter().find(|c| !is_indent(**c)) == Some(&closer) {
        return LineBreak {
            from: col,
            text: format!("\n{inner}\n{indent}"),
            cursor: (1, inner_len),
        };
    }
    LineBreak {
        from: col,
        text: format!("\n{inner}"),
        cursor: (1, inner_len),
    }
}

/// Tab で挿入する空白。1 段がタブならタブ 1 つ、空白なら次の段の位置まで
/// (display_col は行頭からの表示桁)
pub(super) fn tab_fill(unit: &str, display_col: usize) -> String {
    if unit.starts_with('\t') {
        return "\t".to_string();
    }
    let width = unit.len().max(1);
    " ".repeat(width - display_col % width)
}

/// カーソル行の近所からインデント 1 段を推定する。
/// 空白インデントの行が優勢なら「次の行で深くなった幅」の最頻値、タブが優勢ならタブ。
/// 幅 1 の増加は数えない — ブロックコメントの ` * ` や引数の桁揃えで普通に出て、
/// 最小値や最頻値に混ぜると 1 段が空白 1 つと誤推定されるため
pub(super) fn indent_unit(lines: &[String], around: usize) -> String {
    if lines.get(around).is_some_and(|line| line.starts_with('\t')) {
        return "\t".to_string();
    }
    let from = around.saturating_sub(SCAN_LINES);
    let to = around.saturating_add(SCAN_LINES).min(lines.len());
    let mut tab_lines = 0usize;
    let mut space_lines = 0usize;
    // 増加幅 2..=8 の出現回数
    let mut steps = [0usize; 9];
    let mut prev: Option<usize> = None;
    for line in lines.get(from..to).unwrap_or_default() {
        if line.trim().is_empty() {
            continue;
        }
        if line.starts_with('\t') {
            tab_lines += 1;
            prev = None;
            continue;
        }
        let spaces = line.chars().take_while(|c| *c == ' ').count();
        if spaces > 0 {
            space_lines += 1;
        }
        if let Some(p) = prev
            && spaces > p
            && let Some(slot) = steps.get_mut(spaces - p)
        {
            *slot += 1;
        }
        prev = Some(spaces);
    }
    if tab_lines > space_lines {
        return "\t".to_string();
    }
    // 同数なら狭い方 (2 と 4 が混ざるのは 2 段刻みのファイルで 2 段下げた行があるため)
    let best = (2..steps.len())
        .filter(|&w| steps[w] > 0)
        .max_by(|&a, &b| steps[a].cmp(&steps[b]).then(b.cmp(&a)));
    match best {
        Some(width) => " ".repeat(width),
        None => DEFAULT_UNIT.to_string(),
    }
}

fn is_indent(c: char) -> bool {
    c == ' ' || c == '\t'
}

fn closing(c: char) -> Option<char> {
    match c {
        '{' => Some('}'),
        '(' => Some(')'),
        '[' => Some(']'),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_string).collect()
    }

    #[test]
    fn carries_the_indentation_over() {
        let got = line_break("    let x = 1;", 14, "    ");
        assert_eq!(got.text, "\n    ");
        assert_eq!((got.from, got.cursor), (14, (1, 4)));
        // 本文の途中で割った時も、行の残りは新しい行のインデントの後ろへ付く
        let got = line_break("    foo bar", 8, "    ");
        assert_eq!((got.from, got.text.as_str()), (8, "\n    "));
    }

    #[test]
    fn indents_one_level_after_an_opening_bracket() {
        let got = line_break("  fn main() {", 13, "  ");
        assert_eq!(got.text, "\n    ");
        assert_eq!(got.cursor, (1, 4));
        // 対応する閉じ括弧が直後にあれば 3 行に開く
        let got = line_break("\tcall(", 6, "\t");
        assert_eq!(got.text, "\n\t\t");
        let got = line_break("  let v = [];", 11, "  ");
        assert_eq!(got.text, "\n    \n  ");
        assert_eq!(got.cursor, (1, 4));
        // 対応しない閉じ括弧では開かない
        let got = line_break("x = (]", 5, "    ");
        assert_eq!(got.text, "\n    ");
    }

    #[test]
    fn a_blank_indented_line_leaves_no_trailing_spaces() {
        let got = line_break("        ", 8, "    ");
        assert_eq!(got.from, 0);
        assert_eq!(got.text, "\n        ");
        assert_eq!(got.cursor, (1, 8));
        // インデントの途中で押したら、そこまでのインデントだけを引き継ぐ
        let got = line_break("    foo", 2, "    ");
        assert_eq!((got.from, got.text.as_str()), (2, "\n  "));
    }

    #[test]
    fn guesses_the_unit_from_nearby_lines() {
        let two = lines("fn a() {\n  if x {\n    y();\n  }\n}\n");
        assert_eq!(indent_unit(&two, 0), "  ");
        let tabs = lines("fn a() {\n\tif x {\n\t\ty();\n\t}\n}\n");
        assert_eq!(indent_unit(&tabs, 0), "\t");
        // ブロックコメントの ` * ` (幅 1 の増加) に釣られない
        let doc = lines("/**\n * doc\n */\nfn a() {\n    b();\n}\n");
        assert_eq!(indent_unit(&doc, 0), "    ");
        assert_eq!(indent_unit(&lines("plain\ntext\n"), 0), DEFAULT_UNIT);
    }

    #[test]
    fn tab_fills_up_to_the_next_level() {
        assert_eq!(tab_fill("    ", 0), "    ");
        assert_eq!(tab_fill("    ", 5), "   ");
        assert_eq!(tab_fill("  ", 3), " ");
        assert_eq!(tab_fill("\t", 3), "\t");
    }
}
