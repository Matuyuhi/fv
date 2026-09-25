use super::Viewer;
use super::content::Content;

/// 1件のマッチ位置。列は plain の char 単位インデックス (gutter は含まない)
pub struct Match {
    pub line: usize,
    pub start_col: usize,
    pub end_col: usize,
}

pub struct SearchState {
    pub query: String,
    pub matches: Vec<Match>,
    // Enter で確定した後にだけ Some。n/N で動かす現在位置
    pub current: Option<usize>,
}

impl Viewer {
    /// Search 入力中のライブプレビュー。マッチを再計算するだけでジャンプはしない
    pub fn update_search(&mut self, query: &str) {
        if query.is_empty() {
            self.search = None;
            return;
        }
        let matches = match candidate_lines(self.search.as_ref(), query) {
            Some(lines) => self.compute_matches_in(&lines, query),
            None => self.compute_matches(query),
        };
        self.search = Some(SearchState {
            query: query.to_string(),
            matches,
            current: None,
        });
    }

    /// Enter で確定。現在のスクロール位置以降の最初のマッチへジャンプ (なければ先頭へ wrap)
    pub fn confirm_search(&mut self) {
        let Some(search) = &self.search else {
            return;
        };
        if search.matches.is_empty() {
            return;
        }
        let scroll = self.viewport.scroll;
        let idx = search
            .matches
            .iter()
            .position(|m| m.line >= scroll)
            .unwrap_or(0);
        let line = search.matches[idx].line;
        if let Some(search) = &mut self.search {
            search.current = Some(idx);
        }
        self.center_on(line);
    }

    pub fn cancel_search(&mut self) {
        self.search = None;
    }

    /// 横断検索 (Ctrl+f) のヒットへ着地する。同じクエリでファイル内検索を立て、その行の
    /// マッチを現在位置にしてから中央へ寄せる — `/` で同じ語を探した後と同じ状態にする
    /// (n/N が続けて効き、ハイライトも同じ色で出る)。col は plain の char 桁で、同じ行の
    /// 2 つ目以降のヒットを選んだ時に n がその次へ進めるよう、行だけでなく桁まで突き合わせる
    /// (開き直しで行がずれていて見つからなければ、その行以降の最初のマッチへ落とす)
    pub fn locate_search(&mut self, query: &str, line: usize, col: usize) {
        let matches = self.compute_matches(query);
        let current = matches
            .iter()
            .position(|m| m.line == line && m.start_col == col)
            .or_else(|| matches.iter().position(|m| m.line >= line))
            .or_else(|| (!matches.is_empty()).then_some(0));
        self.search = Some(SearchState {
            query: query.to_string(),
            matches,
            current,
        });
        self.center_on(line);
    }

    pub fn next_match(&mut self) {
        self.step_match(1);
    }

    pub fn prev_match(&mut self) {
        self.step_match(-1);
    }

    fn step_match(&mut self, delta: isize) {
        let Some(search) = &self.search else {
            return;
        };
        if search.matches.is_empty() {
            return;
        }
        let Some(current) = search.current else {
            return;
        };
        let len = search.matches.len() as isize;
        let next = (current as isize + delta).rem_euclid(len) as usize;
        let line = search.matches[next].line;
        if let Some(search) = &mut self.search {
            search.current = Some(next);
        }
        self.center_on(line);
    }

    // マッチ行が viewport の中央付近に来るようスクロールする。goto_line (mod.rs) からも呼ばれる。
    // 飛んだ先が行カーソルになる — 検索や :N の着地点が「今どこを見ているか」そのものなので
    pub(super) fn center_on(&mut self, line: usize) {
        let last = self.line_count().saturating_sub(1);
        self.set_cursor(line);
        self.viewport.center_on(line, last);
        self.ensure_cursor_visible();
    }

    fn compute_matches(&self, query: &str) -> Vec<Match> {
        let Some(open) = &self.current else {
            return Vec::new();
        };
        let Content::Text(doc) = open.content.as_ref() else {
            return Vec::new();
        };
        search_matches(&doc.plain, query)
    }

    fn compute_matches_in(&self, lines: &[usize], query: &str) -> Vec<Match> {
        let Some(open) = &self.current else {
            return Vec::new();
        };
        let Content::Text(doc) = open.content.as_ref() else {
            return Vec::new();
        };
        matches_in(
            lines
                .iter()
                .filter_map(|&i| doc.plain.get(i).map(|text| (i, text.as_str()))),
            query,
        )
    }

    // ファイルを開き直した/reload した際、同じクエリでマッチを再計算する。
    // 確定済みだった場合は現在位置を新しいマッチ数に合わせてクランプする
    pub(super) fn recompute_search(&mut self) {
        let Some(query) = self.search.as_ref().map(|s| s.query.clone()) else {
            return;
        };
        let matches = self.compute_matches(&query);
        if let Some(search) = &mut self.search {
            let current = search
                .current
                .map(|idx| idx.min(matches.len().saturating_sub(1)));
            search.current = if matches.is_empty() { None } else { current };
            search.matches = matches;
        }
    }
}

// smart-case (クエリが全て小文字なら大小無視、大文字を含めば区別) の部分一致検索。
// 大小無視の比較は ASCII の範囲だけ行う (to_ascii_lowercase は char 数を変えないため、
// plain の char 列インデックスと桁位置が確実に一致する)。
// GIT レーンの diff 内検索も同じマッチングを再利用するため pub(crate)。diff は plain な
// 文字列を持たないので、呼び出し側が Line の span[1..] を連結して渡す
pub(crate) fn search_matches(plain: &[String], query: &str) -> Vec<Match> {
    matches_in(
        plain.iter().enumerate().map(|(i, text)| (i, text.as_str())),
        query,
    )
}

/// 渡された (行番号, 本文) の組だけを照合する。search_matches の中身で、
/// candidate_lines で絞った行だけを見直す経路と共有する。行番号は昇順で渡すこと
/// (text_pane の highlight_matches が matches の並びを二分探索する)
pub(crate) fn matches_in<'a>(
    lines: impl Iterator<Item = (usize, &'a str)>,
    query: &str,
) -> Vec<Match> {
    if query.is_empty() {
        return Vec::new();
    }
    lines
        .flat_map(|(line, text)| {
            line_matches(text, query).map(move |(start_col, end_col)| Match {
                line,
                start_col,
                end_col,
            })
        })
        .collect()
}

/// 入力中のクエリが直前のクエリを後ろへ伸ばしただけ (1 文字打った・貼り付けた) なら、
/// 新しいマッチは直前にマッチがあった行にしか現れない。その行 (昇順・重複なし) を返す。
/// smart-case の切り替わりでも成り立つ: 伸ばした結果が大文字を含んで大小を区別するように
/// なっても、区別して一致する行は区別せずにも一致する (逆向き — 区別あり → 無視 — は
/// 伸ばしただけでは起きない)。1 打鍵ごとに文書全体を舐め直さないための絞り込み
pub(crate) fn candidate_lines(prev: Option<&SearchState>, query: &str) -> Option<Vec<usize>> {
    let prev = prev?;
    if prev.query.is_empty() || !query.starts_with(prev.query.as_str()) {
        return None;
    }
    let mut lines: Vec<usize> = prev.matches.iter().map(|m| m.line).collect();
    lines.dedup();
    Some(lines)
}

/// 1 行ぶんの一致 (start_col, end_col) を先頭から順に返す。search_matches の行単位の中身で、
/// 横断検索 (component/grep/) が 1 ファイルあたりの上限まで `take` で打ち切れるよう
/// イテレータとして分けてある — 巨大な 1 行 (minified な JS 等) で全マッチを確保してから
/// 捨てる、を避けるため。規則 (smart-case・ASCII 畳み込み) はここが唯一の定義。
///
/// 照合はバイト列のまま行う (行ごとに char 列を確保し直すと、2 万行の文書で 1 打鍵に
/// 数 ms かかっていた)。畳み込みが ASCII 限定なので非 ASCII のバイトは素のまま比べればよく、
/// UTF-8 は先頭バイトと継続バイトが区別できるので、一致は必ず char 境界から始まる —
/// char 列で比べた結果と同じになる。重なる一致も従来どおり全て返す
pub(crate) fn line_matches<'a>(
    text: &'a str,
    query: &'a str,
) -> impl Iterator<Item = (usize, usize)> + 'a {
    let ignore_case = !query.chars().any(|c| c.is_uppercase());
    let hay = text.as_bytes();
    let needle = query.as_bytes();
    let needle_chars = query.chars().count();
    let eq = move |a: &[u8]| {
        if ignore_case {
            a.eq_ignore_ascii_case(needle)
        } else {
            a == needle
        }
    };
    // 先頭バイトが合う位置まで素のバイト比較で読み飛ばす (大半の位置はここで落ちる)。
    // 大小無視なら大文字側も候補にする — needle は小文字しか含まないので畳むのは片側だけでよい
    let first = needle.first().copied().unwrap_or(0);
    let first_alt = if ignore_case {
        first.to_ascii_uppercase()
    } else {
        first
    };
    // 次に試すバイト位置と、そこまでの char 数 (桁)。一致位置の桁は前回からの差分だけ数える
    let mut at = 0usize;
    let mut counted = (0usize, 0usize);
    std::iter::from_fn(move || {
        // 空クエリは何にも一致しない扱い (matches_in の入口でも弾いている)
        if needle.is_empty() {
            return None;
        }
        while at + needle.len() <= hay.len() {
            let last = hay.len() - needle.len();
            let skip = hay[at..=last]
                .iter()
                .position(|&b| b == first || b == first_alt)?;
            let start = at + skip;
            at = start + 1;
            if !eq(&hay[start..start + needle.len()]) {
                continue;
            }
            let (from, col) = counted;
            let col = col + char_starts(&hay[from..start]);
            counted = (start, col);
            return Some((col, col + needle_chars));
        }
        None
    })
}

// UTF-8 の先頭バイト (継続バイト 0b10xx_xxxx 以外) を数える = char 数
fn char_starts(bytes: &[u8]) -> usize {
    bytes.iter().filter(|&&b| (b & 0xC0) != 0x80).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cols(text: &str, query: &str) -> Vec<(usize, usize)> {
        line_matches(text, query).collect()
    }

    #[test]
    fn columns_are_char_indices_even_after_multibyte_text() {
        // バイト位置ではなく char 桁で返す (全角 3 バイトを 1 桁と数える)
        assert_eq!(cols("日本語 foo 日本 foo", "foo"), vec![(4, 7), (11, 14)]);
        assert_eq!(cols("日本語の日本", "日本"), vec![(0, 2), (4, 6)]);
    }

    #[test]
    fn smart_case_folds_only_when_the_query_is_lowercase() {
        assert_eq!(cols("Foo foo FOO", "foo"), vec![(0, 3), (4, 7), (8, 11)]);
        assert_eq!(cols("Foo foo FOO", "Foo"), vec![(0, 3)]);
        // 畳み込みは ASCII 限定 (非 ASCII の大文字は区別したまま)
        assert_eq!(cols("É é", "é"), vec![(2, 3)]);
    }

    #[test]
    fn overlapping_matches_are_all_reported() {
        assert_eq!(cols("aaaa", "aa"), vec![(0, 2), (1, 3), (2, 4)]);
        assert!(cols("a", "aa").is_empty());
        assert!(cols("abc", "").is_empty());
    }

    #[test]
    fn extending_the_query_narrows_to_the_previous_lines() {
        let plain: Vec<String> = ["item one", "other", "item Two", "Item three"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let prev = SearchState {
            query: "item".to_string(),
            matches: search_matches(&plain, "item"),
            current: None,
        };
        assert_eq!(candidate_lines(Some(&prev), "item t"), Some(vec![0, 2, 3]));
        // 縮めた・別の語に変えた時は絞れない (全体を舐め直す)
        assert_eq!(candidate_lines(Some(&prev), "ite"), None);
        assert_eq!(candidate_lines(Some(&prev), "other"), None);
        // 絞った結果は全体を舐め直した結果と一致する (大小を区別するよう切り替わっても)
        for query in ["item t", "item T"] {
            let lines = candidate_lines(Some(&prev), query).unwrap();
            let narrowed = matches_in(lines.iter().map(|&i| (i, plain[i].as_str())), query);
            let full = search_matches(&plain, query);
            let key = |ms: &[Match]| {
                ms.iter()
                    .map(|m| (m.line, m.start_col, m.end_col))
                    .collect::<Vec<_>>()
            };
            assert_eq!(key(&narrowed), key(&full));
        }
    }
}
