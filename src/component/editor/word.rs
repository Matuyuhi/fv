//! 単語境界の計算。Alt/Ctrl+←/→ の移動と Alt+Backspace/Delete の削除が
//! 同じ境界を見るよう、状態を持たない純関数として 1 箇所に置く。
//!
//! 空白の連なりだけを区切りにする WORD 単位ではなく、**文字クラスの切れ目**を境界にする。
//! コードでは `foo.bar(baz)` のような列が普通で、行末まで 1 語として飛ぶと
//! 「単語ごとに動かす」という用途を満たせないため (VSCode / macOS の Option+←→ と同じ挙動)。

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Space,
    /// 英数字・`_`。日本語も is_alphanumeric に含まれるので語として扱う
    Word,
    Punct,
}

fn class(c: char) -> Class {
    if c.is_whitespace() {
        Class::Space
    } else if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else {
        Class::Punct
    }
}

/// col から右へ見て次の境界 (char インデックス)。空白を跨いでから 1 クラス分だけ進む。
/// 行末に達したら line の長さを返す (行を跨ぐかどうかは呼び出し側の判断)
pub(super) fn next_boundary(line: &str, col: usize) -> usize {
    let chars: Vec<char> = line.chars().collect();
    let mut c = col.min(chars.len());
    while c < chars.len() && class(chars[c]) == Class::Space {
        c += 1;
    }
    if c >= chars.len() {
        return chars.len();
    }
    let run = class(chars[c]);
    while c < chars.len() && class(chars[c]) == run {
        c += 1;
    }
    c
}

/// col から左へ見て前の境界 (char インデックス)。行頭に達したら 0 を返す
pub(super) fn prev_boundary(line: &str, col: usize) -> usize {
    let chars: Vec<char> = line.chars().collect();
    let mut c = col.min(chars.len());
    while c > 0 && class(chars[c - 1]) == Class::Space {
        c -= 1;
    }
    if c == 0 {
        return 0;
    }
    let run = class(chars[c - 1]);
    while c > 0 && class(chars[c - 1]) == run {
        c -= 1;
    }
    c
}

/// Home の移動先。インデントの直後と行頭を往復する (VSCode / Emacs と同じ)。
/// 行頭固定にしないのは、コードでは「その行の本文の先頭」に用があることの方が多いため
pub(super) fn home_col(line: &str, col: usize) -> usize {
    let indent = line.chars().take_while(|c| c.is_whitespace()).count();
    if col == indent { 0 } else { indent }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stops_at_class_boundaries() {
        let line = "let foo.bar = baz;";
        // let → 空白を跨いで foo → . → bar
        assert_eq!(next_boundary(line, 0), 3);
        assert_eq!(next_boundary(line, 3), 7);
        assert_eq!(next_boundary(line, 7), 8);
        assert_eq!(next_boundary(line, 8), 11);
    }

    #[test]
    fn skips_leading_indent() {
        let line = "    value";
        assert_eq!(next_boundary(line, 0), 9);
        assert_eq!(prev_boundary(line, 9), 4);
        assert_eq!(prev_boundary(line, 4), 0);
    }

    #[test]
    fn saturates_at_line_ends() {
        let line = "ab";
        assert_eq!(next_boundary(line, 2), 2);
        assert_eq!(next_boundary(line, 99), 2);
        assert_eq!(prev_boundary(line, 0), 0);
    }

    #[test]
    fn counts_multibyte_as_chars() {
        // 日本語は語として扱い、記号との境界で止まる (char 数で数える)
        let line = "変数名 = 値;";
        assert_eq!(next_boundary(line, 0), 3);
        assert_eq!(next_boundary(line, 3), 5);
        assert_eq!(prev_boundary(line, 5), 4);
    }

    #[test]
    fn home_toggles_between_indent_and_column_zero() {
        let line = "\tif x {";
        assert_eq!(home_col(line, 4), 1);
        assert_eq!(home_col(line, 1), 0);
        assert_eq!(home_col(line, 0), 1);
    }
}
