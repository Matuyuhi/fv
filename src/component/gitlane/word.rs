//! word-level ハイライト (#29) の範囲計算。hunk 内で「連続する削除ブロック → 直後の連続する
//! 追加ブロック」を対応付け、行単位の LCS (editor::diff) を char 単位に流用して差分範囲を出す。
//! 対応が取れなかった行は None のままにし、呼び出し側を従来の全行ハイライトへ倒す。

use crate::component::editor::diff::{self, CharRanges};
use crate::text;

use super::{Kind, MAX_WORD_DIFF_PAIRS_PER_HUNK};

// 1 対 1 で char 単位の差分 (editor::diff::word_diff) を計算する。行数が合わない・
// 打ち切り上限を超えるペアは None のままにし、呼び出し側を従来の全行ハイライトに倒す
pub(super) fn word_diff_ranges(body: &[(Kind, &str)]) -> Vec<Option<CharRanges>> {
    let mut ranges: Vec<Option<CharRanges>> = vec![None; body.len()];
    let mut hunk_pairs = 0usize;
    let mut i = 0;
    while i < body.len() {
        match body[i].0 {
            Kind::Hunk => {
                hunk_pairs = 0;
                i += 1;
            }
            Kind::Deleted => {
                let del_start = i;
                let del_end = run_end(body, del_start, |k| matches!(k, Kind::Deleted));
                let add_start = del_end;
                let add_end = run_end(body, add_start, |k| matches!(k, Kind::Added));
                let del_len = del_end - del_start;
                let add_len = add_end - add_start;
                if del_len == add_len && hunk_pairs + del_len <= MAX_WORD_DIFF_PAIRS_PER_HUNK {
                    hunk_pairs += del_len;
                    for offset in 0..del_len {
                        pair_word_diff(body, del_start + offset, add_start + offset, &mut ranges);
                    }
                } else {
                    hunk_pairs += del_len;
                }
                i = add_end;
            }
            _ => i += 1,
        }
    }
    ranges
}

pub(super) fn run_end(
    body: &[(Kind, &str)],
    start: usize,
    matches_kind: impl Fn(&Kind) -> bool,
) -> usize {
    let mut j = start;
    while j < body.len() && matches_kind(&body[j].0) {
        j += 1;
    }
    j
}

// 削除行・追加行 1 組の char diff を計算し、結果を該当 index の ranges に入れる。
// 先頭 1 文字は diff の +/- マーカーなので比較対象から外し、range だけマーカー分 (1 char)
// 戻して content 上の座標に合わせる
fn pair_word_diff(
    body: &[(Kind, &str)],
    del_idx: usize,
    add_idx: usize,
    ranges: &mut [Option<CharRanges>],
) {
    let del_body = text::normalize(&body[del_idx].1[1..]);
    let add_body = text::normalize(&body[add_idx].1[1..]);
    let Some((del_ranges, add_ranges)) = diff::word_diff(&del_body, &add_body) else {
        return;
    };
    ranges[del_idx] = Some(shift_ranges(del_ranges));
    ranges[add_idx] = Some(shift_ranges(add_ranges));
}

fn shift_ranges(ranges: CharRanges) -> CharRanges {
    // マーカー1 文字分だけ後ろにずらす (word_diff は marker を含まない文字列で計算している)
    ranges.into_iter().map(|(s, e)| (s + 1, e + 1)).collect()
}
