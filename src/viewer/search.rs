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
        let matches = self.compute_matches(query);
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
        let scroll = self.scroll;
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

    // マッチ行が viewport の中央付近に来るようスクロールする。goto_line (mod.rs) からも呼ばれる
    pub(super) fn center_on(&mut self, line: usize) {
        let last = self.line_count().saturating_sub(1);
        let half = self.viewport_height / 2;
        self.scroll = line.saturating_sub(half).min(last);
    }

    fn compute_matches(&self, query: &str) -> Vec<Match> {
        let Some(open) = &self.current else {
            return Vec::new();
        };
        let Content::Text { plain, .. } = open.content.as_ref() else {
            return Vec::new();
        };
        search_matches(plain, query)
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
// plain の char 列インデックスと桁位置が確実に一致する)
fn search_matches(plain: &[String], query: &str) -> Vec<Match> {
    if query.is_empty() {
        return Vec::new();
    }
    let ignore_case = !query.chars().any(|c| c.is_uppercase());
    let needle: Vec<char> = fold_case(query, ignore_case).collect();
    let mut matches = Vec::new();
    for (line, text) in plain.iter().enumerate() {
        let haystack: Vec<char> = fold_case(text, ignore_case).collect();
        if haystack.len() < needle.len() {
            continue;
        }
        for start in 0..=(haystack.len() - needle.len()) {
            if haystack[start..start + needle.len()] == needle[..] {
                matches.push(Match {
                    line,
                    start_col: start,
                    end_col: start + needle.len(),
                });
            }
        }
    }
    matches
}

fn fold_case(s: &str, ignore_case: bool) -> impl Iterator<Item = char> + '_ {
    s.chars().map(move |c| {
        if ignore_case {
            c.to_ascii_lowercase()
        } else {
            c
        }
    })
}
