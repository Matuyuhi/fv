//! 行単位 stage/unstage (`S`、GIT レーンの diff ペイン) のパッチ組み立て。
//!
//! hunk 単位 (`Space`) は「その hunk の生行をそのまま連結するだけ」で済むが、行単位は
//! 選ばなかった変更行を落とす/文脈行へ落とすという書き換えが要るので、生 diff を読み直して
//! hunk header を作り直す。作法は `git add -p` の行選択と同じ:
//!
//! - stage (順方向 apply): 選ばなかった `+` は落とす / 選ばなかった `-` は文脈行にする
//!   (pre-image は worktree ではなく index 側の親、つまり「選んだ削除だけを反映する」)
//! - unstage (`--reverse` apply): pre-image が新側 (index) になるので向きが反転し、
//!   選ばなかった `+` は文脈行 / 選ばなかった `-` は落とす
//!
//! hunk header の開始行番号は書き換えない (hunk 単位と同じ理由: git apply は文脈行を
//! 照合して適用位置を決めるのでオフセットは吸収される)。行数だけは書き換えた本文と
//! 食い違うと git がパッチを弾くので必ず数え直す。

use super::render::{hunk_old_start, hunk_start};

/// 選択範囲 (生 diff 上の行 index の閉区間) に含まれる変更行だけを反映するパッチを組み立てる。
/// 選択が変更行を 1 つも含まない場合は None (呼び出し側が notice に倒す)
pub(super) fn build_line_patch(
    raw: &[String],
    raw_hunks: &[usize],
    lo: usize,
    hi: usize,
    reverse: bool,
) -> Option<String> {
    let header_end = *raw_hunks.first()?;
    let mut body = String::new();
    for (i, &start) in raw_hunks.iter().enumerate() {
        let end = raw_hunks.get(i + 1).copied().unwrap_or(raw.len());
        if let Some(hunk) = transform_hunk(&raw[start..end], start, lo, hi, reverse) {
            body.push_str(&hunk);
        }
    }
    if body.is_empty() {
        return None;
    }
    let mut patch = String::new();
    for line in &raw[..header_end] {
        patch.push_str(line);
        patch.push('\n');
    }
    patch.push_str(&body);
    Some(patch)
}

/// 1 hunk 分を書き換える。選択された変更行を含まない hunk は None (パッチから丸ごと落とす)
fn transform_hunk(
    hunk: &[String],
    offset: usize,
    lo: usize,
    hi: usize,
    reverse: bool,
) -> Option<String> {
    let header = hunk.first()?;
    let mut body: Vec<String> = Vec::with_capacity(hunk.len());
    let mut picked = 0usize;
    // "\ No newline at end of file" は直前の行に紐づく注記なので、その行を落としたら一緒に落とす
    let mut kept_previous = true;
    for (i, line) in hunk.iter().enumerate().skip(1) {
        let selected = (offset + i) >= lo && (offset + i) <= hi;
        // keep = パッチに残すか / converted = 残すが文脈行へ落とすか
        let (keep, converted) = match line.as_bytes().first() {
            Some(b'+') => (selected || reverse, !selected),
            Some(b'-') => (selected || !reverse, !selected),
            Some(b'\\') => {
                if kept_previous {
                    body.push(line.clone());
                }
                continue;
            }
            // 文脈行 (先頭が空白、空行は git が空文字列で出す) はそのまま残す。
            // 選択範囲に入っていても「反映する変更」にはならないので picked には数えない
            _ => {
                kept_previous = true;
                body.push(line.clone());
                continue;
            }
        };
        kept_previous = keep;
        if !keep {
            continue;
        }
        if converted {
            // 選ばなかった変更行を文脈行へ落とす (マーカーだけを空白に差し替える)
            body.push(format!(" {}", &line[1..]));
        } else {
            picked += 1;
            body.push(line.clone());
        }
    }
    if picked == 0 {
        return None;
    }

    let (old, new) = counts(&body);
    let old_start = hunk_old_start(header).unwrap_or(1);
    let new_start = hunk_start(header).unwrap_or(1);
    // "@@ -a,b +c,d @@ <関数名>" の 3 つ目のフィールド (関数名) はそのまま引き継ぐ
    let heading = header.split("@@").nth(2).unwrap_or("");
    let mut out = format!("@@ -{old_start},{old} +{new_start},{new} @@{heading}\n");
    for line in body {
        out.push_str(&line);
        out.push('\n');
    }
    Some(out)
}

// 書き換え後の本文から hunk header の行数を数え直す。注記行 (\) はどちら側にも数えない
fn counts(body: &[String]) -> (usize, usize) {
    let mut old = 0usize;
    let mut new = 0usize;
    for line in body {
        match line.as_bytes().first() {
            Some(b'+') => new += 1,
            Some(b'-') => old += 1,
            Some(b'\\') => {}
            _ => {
                old += 1;
                new += 1;
            }
        }
    }
    (old, new)
}

#[cfg(test)]
mod tests {
    use super::build_line_patch;

    fn raw(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|l| l.to_string()).collect()
    }

    // raw:              0            1            2          3      4      5      6
    const SAMPLE: &[&str] = &[
        "--- a/x.rs",
        "+++ b/x.rs",
        "@@ -1,3 +1,4 @@ fn main",
        " ctx",
        "-old",
        "+new1",
        "+new2",
    ];

    // stage: 選ばなかった + は落とし、選ばなかった - は文脈行にする。
    // 行数は書き換え後の本文から数え直さないと git がパッチを弾く
    #[test]
    fn staging_one_added_line_drops_the_other_and_keeps_the_deletion_as_context() {
        let patch = build_line_patch(&raw(SAMPLE), &[2], 5, 5, false).unwrap();
        assert_eq!(
            patch,
            "--- a/x.rs\n+++ b/x.rs\n@@ -1,2 +1,3 @@ fn main\n ctx\n old\n+new1\n"
        );
    }

    // unstage は --reverse apply なので pre-image が新側になり、向きが反転する
    #[test]
    fn unstaging_one_added_line_keeps_the_other_as_context() {
        let patch = build_line_patch(&raw(SAMPLE), &[2], 5, 5, true).unwrap();
        assert_eq!(
            patch,
            "--- a/x.rs\n+++ b/x.rs\n@@ -1,2 +1,3 @@ fn main\n ctx\n+new1\n new2\n"
        );
    }

    // 選択が変更行を 1 つも含まない (文脈行だけ) なら組み立てない
    #[test]
    fn selecting_only_context_lines_builds_nothing() {
        assert!(build_line_patch(&raw(SAMPLE), &[2], 3, 3, false).is_none());
    }

    // 選択を含まない hunk はパッチから丸ごと落とす (先行 hunk が未適用でも文脈で吸収される)
    #[test]
    fn hunks_without_a_selected_line_are_dropped() {
        let lines = raw(&[
            "--- a/x.rs",
            "+++ b/x.rs",
            "@@ -1,2 +1,2 @@",
            " a",
            "-b",
            "+B",
            "@@ -10,2 +10,2 @@",
            " c",
            "-d",
            "+D",
        ]);
        let patch = build_line_patch(&lines, &[2, 6], 9, 9, false).unwrap();
        assert_eq!(
            patch,
            "--- a/x.rs\n+++ b/x.rs\n@@ -10,2 +10,3 @@\n c\n d\n+D\n"
        );
    }

    // 落とした行にぶら下がる "\\ No newline" も一緒に落とす (注記は直前の行に紐づくため)
    #[test]
    fn a_no_newline_note_is_dropped_with_the_line_it_belongs_to() {
        let lines = raw(&[
            "--- a/x.rs",
            "+++ b/x.rs",
            "@@ -1,1 +1,3 @@",
            " ctx",
            "+new1",
            "+new2",
            "\\ No newline at end of file",
        ]);
        let patch = build_line_patch(&lines, &[2], 4, 4, false).unwrap();
        assert_eq!(
            patch,
            "--- a/x.rs\n+++ b/x.rs\n@@ -1,1 +1,2 @@\n ctx\n+new1\n"
        );
    }
}
