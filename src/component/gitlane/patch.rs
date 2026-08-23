//! 行単位 stage / unstage (`Enter`) のパッチ組み立て。
//!
//! hunk 単位 (`Space`) は「生 diff のその区間をそのまま切り出す」だけで済むが、行単位は
//! 選択しなかった変更行を**適用先に合わせて書き換える**必要がある。`git apply --cached` は
//! パッチの片側 (forward なら旧側 / `--reverse` なら新側) が index の内容と一致することを
//! 文脈照合で確かめるので、「index に在る行」は必ずパッチに残さなければならない:
//!
//! - forward (stage): index はまだ変更前なので、未選択の `-` は index に**在る** → 文脈化。
//!   未選択の `+` は index に**無い** → 落とす
//! - reverse (unstage): index は変更後なので、未選択の `+` が index に**在る** → 文脈化。
//!   未選択の `-` は index に**無い** → 落とす
//!
//! 書き換えで行数が変わるため hunk header (`@@ -a,b +c,d @@`) の b/d は数え直す。開始行
//! (a/c) は元のまま据え置く — `git apply` は文脈行を照合して適用位置を決めるので、先行する
//! hunk が未適用でもオフセットを吸収する (`current_hunk_patch` が行番号を書き換えないのと同じ作法)。

use std::collections::BTreeSet;

use super::render::{hunk_old_start, hunk_start};

/// 選択行だけを含む 1 ファイル分のパッチ。`lines` は実際に反映される変更行数 (notice 用)。
/// 呼び出し側の `GitState::current_line_patch` が返す `LinePatch` (断りの理由まで持つ enum)
/// とは別物で、こちらは「組み立てに成功した結果」だけを表す
#[derive(Debug)]
pub(super) struct BuiltPatch {
    pub patch: String,
    pub lines: usize,
}

/// 組み立てを断る理由。`build_line_patch` は生 diff のファイルヘッダをそのまま前置きするので、
/// ヘッダが「ファイルまるごとの操作」を宣言している diff とは噛み合わないことがある
#[derive(Debug, PartialEq)]
pub(super) enum PatchError {
    /// 反映対象の変更行が 1 つも無い
    Empty,
    /// rename / copy (`diff.renames=copies` 等)。`--- a/old` と `+++ b/new` が別のパスを
    /// 指し、部分適用で片方だけを動かせない。copy はさらに、メタデータを残したまま部分
    /// パッチを当てるとコピー元の内容が丸ごと index に作られうる
    Rename,
    /// 新規/削除ファイルで、片側が `/dev/null` なのにその側へ行が残る組み合わせ。
    /// 例: staged の新規ファイルから 1 行だけ unstage する (未選択の `+` を文脈化すると
    /// 旧側 = /dev/null に内容が出てしまう)
    DevNullSideNotEmpty,
}

/// `raw` (1 ファイル分の生 unified diff) から、`selected` (raw の index 集合) に含まれる
/// 変更行だけを反映するパッチを組み立てる。選択行を 1 つも含まない hunk は丸ごと出力しない
/// (空の hunk を残すと git apply がエラーにするため)。反映対象が 0 行なら None
pub(super) fn build_line_patch(
    raw: &[String],
    raw_hunks: &[usize],
    selected: &BTreeSet<usize>,
    reverse: bool,
) -> Result<BuiltPatch, PatchError> {
    let Some(&header_end) = raw_hunks.first() else {
        return Err(PatchError::Empty);
    };
    let header = &raw[..header_end];
    // rename / copy は `--- a/old` と `+++ b/new` が別パスなので、行の部分適用と噛み合わない
    // (ファイル単位の Space なら丸ごと動かせる)
    if header
        .iter()
        .any(|l| l.starts_with("rename from ") || l.starts_with("copy from "))
    {
        return Err(PatchError::Rename);
    }

    let mut patch = String::new();
    for line in header {
        // mode 変更だけのメタ行は落とす。行を 1 本 stage したいだけなのに実行ビットまで
        // index へ移してしまうのは、選んでいない変更を黙って混ぜることになる
        if line.starts_with("old mode ") || line.starts_with("new mode ") {
            continue;
        }
        patch.push_str(line);
        patch.push('\n');
    }

    let mut applied = 0usize;
    let mut total_old = 0usize;
    let mut total_new = 0usize;
    for (i, &start) in raw_hunks.iter().enumerate() {
        let end = raw_hunks.get(i + 1).copied().unwrap_or(raw.len());
        let body = &raw[start + 1..end];
        if !(start + 1..end).any(|j| selected.contains(&j)) {
            continue;
        }
        let hunk = transform_hunk(body, start + 1, selected, reverse);
        applied += hunk.applied;
        total_old += hunk.old_count;
        total_new += hunk.new_count;
        patch.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            hunk_old_start(&raw[start]).unwrap_or(1),
            hunk.old_count,
            hunk_start(&raw[start]).unwrap_or(1),
            hunk.new_count,
        ));
        for line in hunk.lines {
            patch.push_str(&line);
            patch.push('\n');
        }
    }

    if applied == 0 {
        return Err(PatchError::Empty);
    }
    // 新規ファイル (`--- /dev/null`) / 削除ファイル (`+++ /dev/null`) は、その側に 1 行も
    // 出ない時だけ整合する。文脈化 (未選択の変更行を残す処理) が /dev/null 側に行を作る
    // 組み合わせは git apply が受け付けないので、壊れたパッチを投げる前にここで断る
    if header.iter().any(|l| l == "--- /dev/null") && total_old > 0 {
        return Err(PatchError::DevNullSideNotEmpty);
    }
    if header.iter().any(|l| l == "+++ /dev/null") && total_new > 0 {
        return Err(PatchError::DevNullSideNotEmpty);
    }
    Ok(BuiltPatch {
        patch,
        lines: applied,
    })
}

struct Hunk {
    lines: Vec<String>,
    old_count: usize,
    new_count: usize,
    applied: usize,
}

fn transform_hunk(
    body: &[String],
    offset: usize,
    selected: &BTreeSet<usize>,
    reverse: bool,
) -> Hunk {
    let mut out = Hunk {
        lines: Vec::with_capacity(body.len()),
        old_count: 0,
        new_count: 0,
        applied: 0,
    };
    // 直前の行を残したか。"\ No newline at end of file" は直前の行に付随する注記なので、
    // その行を落としたなら注記も一緒に落とす
    let mut kept_previous = false;
    for (i, line) in body.iter().enumerate() {
        match line.as_bytes().first() {
            Some(&c @ (b'+' | b'-')) => {
                let added = c == b'+';
                if selected.contains(&(offset + i)) {
                    out.lines.push(line.clone());
                    if added {
                        out.new_count += 1;
                    } else {
                        out.old_count += 1;
                    }
                    out.applied += 1;
                    kept_previous = true;
                } else if added == reverse {
                    // 適用先に既に在る行なので文脈として残す (落とすと照合が合わない)
                    out.lines.push(format!(" {}", &line[1..]));
                    out.old_count += 1;
                    out.new_count += 1;
                    kept_previous = true;
                } else {
                    kept_previous = false;
                }
            }
            Some(b'\\') => {
                if kept_previous {
                    out.lines.push(line.clone());
                }
            }
            // 文脈行 (先頭が空白)。git は空行の文脈を空文字列で出すので None もここに入る
            _ => {
                out.lines.push(line.clone());
                out.old_count += 1;
                out.new_count += 1;
                kept_previous = true;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{BTreeSet, PatchError, build_line_patch};

    fn raw() -> Vec<String> {
        [
            "diff --git a/a.txt b/a.txt",
            "index 1111111..2222222 100644",
            "--- a/a.txt",
            "+++ b/a.txt",
            "@@ -1,4 +1,4 @@",
            " ctx1",
            "-old1",
            "-old2",
            "+new1",
            "+new2",
            " ctx2",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    fn hunks(raw: &[String]) -> Vec<usize> {
        raw.iter()
            .enumerate()
            .filter(|(_, l)| l.starts_with("@@"))
            .map(|(i, _)| i)
            .collect()
    }

    // stage: 未選択の `-` は index に在るので文脈化、未選択の `+` は index に無いので落とす
    #[test]
    fn staging_one_added_line_contextualizes_the_deletions() {
        let raw = raw();
        let selected: BTreeSet<usize> = [8].into_iter().collect();
        let patch = build_line_patch(&raw, &hunks(&raw), &selected, false).unwrap();
        assert_eq!(patch.lines, 1);
        assert_eq!(
            patch.patch,
            concat!(
                "diff --git a/a.txt b/a.txt\n",
                "index 1111111..2222222 100644\n",
                "--- a/a.txt\n",
                "+++ b/a.txt\n",
                "@@ -1,4 +1,5 @@\n",
                " ctx1\n",
                " old1\n",
                " old2\n",
                "+new1\n",
                " ctx2\n",
            )
        );
    }

    // unstage (reverse): 向きが反転し、未選択の `+` が文脈になる
    #[test]
    fn unstaging_one_deleted_line_contextualizes_the_additions() {
        let raw = raw();
        let selected: BTreeSet<usize> = [6].into_iter().collect();
        let patch = build_line_patch(&raw, &hunks(&raw), &selected, true).unwrap();
        assert_eq!(patch.lines, 1);
        assert_eq!(
            patch.patch,
            concat!(
                "diff --git a/a.txt b/a.txt\n",
                "index 1111111..2222222 100644\n",
                "--- a/a.txt\n",
                "+++ b/a.txt\n",
                "@@ -1,5 +1,4 @@\n",
                " ctx1\n",
                "-old1\n",
                " new1\n",
                " new2\n",
                " ctx2\n",
            )
        );
    }

    // 選択行を含まない hunk は丸ごと落とす (空 hunk を残すと git apply が弾く)
    #[test]
    fn hunks_without_a_selected_line_are_dropped() {
        let mut raw = raw();
        raw.extend(
            ["@@ -20,2 +20,2 @@", "-gone", "+kept"]
                .iter()
                .map(|s| s.to_string()),
        );
        let selected: BTreeSet<usize> = [8].into_iter().collect();
        let patch = build_line_patch(&raw, &hunks(&raw), &selected, false).unwrap();
        assert_eq!(patch.patch.matches("@@ ").count(), 1);
        assert!(!patch.patch.contains("kept"));
    }

    // 複数 hunk に跨る選択は、それぞれの hunk を書き換えて 1 つのパッチに連結する
    #[test]
    fn a_selection_spanning_two_hunks_emits_both() {
        let mut raw = raw();
        raw.extend(
            ["@@ -20,2 +20,2 @@", " ctx3", "+tail"]
                .iter()
                .map(|s| s.to_string()),
        );
        let selected: BTreeSet<usize> = [8, 13].into_iter().collect();
        let patch = build_line_patch(&raw, &hunks(&raw), &selected, false).unwrap();
        assert_eq!(patch.lines, 2);
        assert!(patch.patch.ends_with("@@ -20,1 +20,2 @@\n ctx3\n+tail\n"));
    }

    // 末尾改行なしの注記は、付随する行を落としたら一緒に落とす
    #[test]
    fn the_no_newline_note_follows_the_line_it_belongs_to() {
        let raw: Vec<String> = [
            "--- a/a.txt",
            "+++ b/a.txt",
            "@@ -1,1 +1,1 @@",
            "-old",
            "\\ No newline at end of file",
            "+new",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        // `+new` だけを stage → `-old` は文脈化されるので注記もそのまま残る
        let selected: BTreeSet<usize> = [5].into_iter().collect();
        let patch = build_line_patch(&raw, &hunks(&raw), &selected, false).unwrap();
        assert!(patch.patch.contains(" old\n\\ No newline at end of file\n"));
        // 逆向きでは `-old` が落ちるので注記も消える
        let patch = build_line_patch(&raw, &hunks(&raw), &selected, true).unwrap();
        assert!(!patch.patch.contains("No newline"));
    }

    #[test]
    fn selecting_nothing_yields_no_patch() {
        let raw = raw();
        assert_eq!(
            build_line_patch(&raw, &hunks(&raw), &BTreeSet::new(), false).unwrap_err(),
            PatchError::Empty
        );
    }

    fn new_file() -> Vec<String> {
        [
            "diff --git a/n.txt b/n.txt",
            "new file mode 100644",
            "index 0000000..1111111",
            "--- /dev/null",
            "+++ b/n.txt",
            "@@ -0,0 +1,2 @@",
            "+one",
            "+two",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    // 新規ファイルの一部を stage するのは成立する (未選択の `+` は落とすだけなので
    // 旧側は /dev/null のまま空)
    #[test]
    fn staging_part_of_a_new_file_keeps_the_old_side_empty() {
        let raw = new_file();
        let selected: BTreeSet<usize> = [6].into_iter().collect();
        let patch = build_line_patch(&raw, &hunks(&raw), &selected, false).unwrap();
        assert!(patch.patch.contains("@@ -0,0 +1,1 @@\n+one\n"));
        assert!(!patch.patch.contains("two"));
    }

    // 逆向き (staged の新規ファイルから 1 行だけ unstage) は、未選択の `+` を文脈化する
    // 都合で旧側 = /dev/null に行が出てしまうので組み立てない
    #[test]
    fn unstaging_part_of_a_new_file_is_refused() {
        let raw = new_file();
        let selected: BTreeSet<usize> = [6].into_iter().collect();
        assert_eq!(
            build_line_patch(&raw, &hunks(&raw), &selected, true).unwrap_err(),
            PatchError::DevNullSideNotEmpty
        );
    }

    // rename / copy は旧側と新側でパスが違うので行だけを切り出せない
    // (copy は diff.renames=copies や -C を付けた時に出る)
    #[test]
    fn a_rename_or_copy_is_refused() {
        for verb in ["rename", "copy"] {
            let mut raw: Vec<String> = [
                "diff --git a/old.txt b/new.txt".to_string(),
                "similarity index 90%".to_string(),
                format!("{verb} from old.txt"),
                format!("{verb} to new.txt"),
                "--- a/old.txt".to_string(),
                "+++ b/new.txt".to_string(),
            ]
            .into_iter()
            .collect();
            raw.extend(
                ["@@ -1,1 +1,1 @@", "-a", "+b"]
                    .iter()
                    .map(|s| s.to_string()),
            );
            let selected: BTreeSet<usize> = [8].into_iter().collect();
            assert_eq!(
                build_line_patch(&raw, &hunks(&raw), &selected, false).unwrap_err(),
                PatchError::Rename,
                "{verb}"
            );
        }
    }

    // 実行ビットの変更は「行を 1 本 stage したい」に含まれないので、ヘッダから落とす
    #[test]
    fn a_mode_change_is_not_carried_along_with_a_line() {
        let mut raw: Vec<String> = [
            "diff --git a/a.txt b/a.txt",
            "old mode 100644",
            "new mode 100755",
            "--- a/a.txt",
            "+++ b/a.txt",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        raw.extend(
            ["@@ -1,1 +1,1 @@", "-a", "+b"]
                .iter()
                .map(|s| s.to_string()),
        );
        let selected: BTreeSet<usize> = [7].into_iter().collect();
        let patch = build_line_patch(&raw, &hunks(&raw), &selected, false).unwrap();
        assert!(!patch.patch.contains("mode 100755"));
        assert!(patch.patch.contains("+b\n"));
    }
}
