use std::collections::HashSet;

// LCS の DP がこのセル数を超える編集は諦めて中間領域全体を変更扱いにする
// (共通の前置き・後置きを剥がした後なので、通常の編集でここに達することはまずない)
const MAX_LCS_CELLS: usize = 1_000_000;

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

// current 側の各行が LCS (baseline と共通の行並び) に含まれるかを返す
fn lcs_matched(base: &[String], cur: &[String]) -> Vec<bool> {
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
    let mut matched = vec![false; m];
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if base[i] == cur[j] {
            matched[j] = true;
            i += 1;
            j += 1;
        } else if dp[idx(i + 1, j)] >= dp[idx(i, j + 1)] {
            i += 1;
        } else {
            j += 1;
        }
    }
    matched
}
