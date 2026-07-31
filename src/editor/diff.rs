use std::collections::HashSet;

// LCS の DP がこのセル数を超える編集は諦めて中間領域全体を変更扱いにする
// (共通の前置き・後置きを剥がした後なので、通常の編集でここに達することはまずない)
const MAX_LCS_CELLS: usize = 1_000_000;

// word-level diff (#29) で 1 行に許す char 数上限。行の対応付け自体は gitview 側で
// 行数一致を条件に絞っているが、1 行が長すぎると DP が O(n*m) で重くなるためここで打ち切る。
// 超えた場合は None を返し、呼び出し側は従来の全行ハイライトに倒す
const MAX_WORD_DIFF_CHARS: usize = 500;

/// 1 行内の変更 char range (start, end) の列。word_diff の戻り値を素通しさせず
/// 型に名前を付ける (clippy::type_complexity 回避も兼ねる)
pub(crate) type CharRanges = Vec<(usize, usize)>;

/// baseline に対する current の追加・変更行 (1-origin) を返す。
/// 削除のみの箇所は current 側に行が無いため何も付かない (git diff -U0 の +側と同じ扱い)
pub fn changed_lines(baseline: &[String], current: &[String]) -> HashSet<usize> {
    // 編集は局所的なことが多いので、共通 prefix/suffix を O(n) で剥がして
    // DP を実際に編集された中間領域だけに絞る
    let mut prefix = 0;
    while prefix < baseline.len() && prefix < current.len() && baseline[prefix] == current[prefix] {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < baseline.len() - prefix
        && suffix < current.len() - prefix
        && baseline[baseline.len() - 1 - suffix] == current[current.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let mid_base = &baseline[prefix..baseline.len() - suffix];
    let mid_cur = &current[prefix..current.len() - suffix];

    let mut changed = HashSet::new();
    if mid_cur.is_empty() {
        return changed;
    }
    let matched = if mid_base.is_empty() || mid_base.len() * mid_cur.len() > MAX_LCS_CELLS {
        vec![false; mid_cur.len()]
    } else {
        lcs_matched(mid_base, mid_cur)
    };
    for (i, ok) in matched.iter().enumerate() {
        if !ok {
            changed.insert(prefix + i + 1);
        }
    }
    changed
}

// current 側の各行が LCS (baseline と共通の行並び) に含まれるかを返す。
// 行単位専用の薄いラッパー (要素比較は String の PartialEq)
fn lcs_matched(base: &[String], cur: &[String]) -> Vec<bool> {
    lcs_align(base, cur).1
}

/// base/cur の要素列から LCS を求め、双方の要素が LCS (＝変更されていない共通部分) に
/// 含まれるかを返す。行の String 列 (changed_lines) と 1 行の char 列 (word_diff) の
/// どちらもこの一つの実装を共有する (新しいアルゴリズムを増やさない)
fn lcs_align<T: PartialEq>(base: &[T], cur: &[T]) -> (Vec<bool>, Vec<bool>) {
    let (n, m) = (base.len(), cur.len());
    let idx = |i: usize, j: usize| i * (m + 1) + j;
    let mut dp = vec![0u32; (n + 1) * (m + 1)];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[idx(i, j)] = if base[i] == cur[j] {
                dp[idx(i + 1, j + 1)] + 1
            } else {
                dp[idx(i + 1, j)].max(dp[idx(i, j + 1)])
            };
        }
    }
    let mut base_matched = vec![false; n];
    let mut cur_matched = vec![false; m];
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if base[i] == cur[j] {
            base_matched[i] = true;
            cur_matched[j] = true;
            i += 1;
            j += 1;
        } else if dp[idx(i + 1, j)] >= dp[idx(i, j + 1)] {
            i += 1;
        } else {
            j += 1;
        }
    }
    (base_matched, cur_matched)
}

/// 対応付けられた削除行・追加行の文字列ペアから、双方で「共通部分に含まれない」
/// char range (start, end) の列を返す ((削除行側, 追加行側) の順)。
/// 行が長すぎる場合は None を返し、呼び出し側は word-level ハイライトを諦めて
/// 従来の全行ハイライトにフォールバックする (計算量の打ち切り)
pub(crate) fn word_diff(base: &str, cur: &str) -> Option<(CharRanges, CharRanges)> {
    let base_chars: Vec<char> = base.chars().collect();
    let cur_chars: Vec<char> = cur.chars().collect();
    if base_chars.len() > MAX_WORD_DIFF_CHARS || cur_chars.len() > MAX_WORD_DIFF_CHARS {
        return None;
    }

    // changed_lines と同じく、共通の前置き・後置きは DP に回さない
    let mut prefix = 0;
    while prefix < base_chars.len()
        && prefix < cur_chars.len()
        && base_chars[prefix] == cur_chars[prefix]
    {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < base_chars.len() - prefix
        && suffix < cur_chars.len() - prefix
        && base_chars[base_chars.len() - 1 - suffix] == cur_chars[cur_chars.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let mid_base = &base_chars[prefix..base_chars.len() - suffix];
    let mid_cur = &cur_chars[prefix..cur_chars.len() - suffix];

    let (base_matched, cur_matched) = lcs_align(mid_base, mid_cur);
    Some((
        unmatched_ranges(&base_matched, prefix),
        unmatched_ranges(&cur_matched, prefix),
    ))
}

// matched (LCS に含まれるか) が false の区間をまとめて (start, end) の char range にする。
// offset は prefix トリムで剥がした先頭分のずれを戻すため
fn unmatched_ranges(matched: &[bool], offset: usize) -> CharRanges {
    let mut ranges = Vec::new();
    let mut start: Option<usize> = None;
    for (i, &ok) in matched.iter().enumerate() {
        match (ok, start) {
            (false, None) => start = Some(i),
            (true, Some(s)) => {
                ranges.push((offset + s, offset + i));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        ranges.push((offset + s, offset + matched.len()));
    }
    ranges
}
