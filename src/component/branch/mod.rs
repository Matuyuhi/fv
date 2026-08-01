// `b` ブランチ一覧オーバーレイの状態。Finder (component/finder/mod.rs) と同じ「絞り込み候補 + 選択位置」の
// パターンだが、表示に current マーク・upstream・相対日時・件名を必要とし Finder の
// `candidate: String` だけでは表現できないため専用の状態にする。マッチングだけは
// 新しいマッチャを書かず component/finder/mod.rs の fuzzy_match をそのまま再利用する。
pub mod view;

use ratatui::widgets::ListState;
use std::path::Path;

use crate::component::finder::fuzzy_match;
use crate::git::{self, BranchEntry};

/// 一覧の1行。current は BranchState 構築時に渡された現在ブランチ名との突き合わせで決まる
/// (detached HEAD や非 git repo では current は常に false)
pub struct BranchRow {
    pub entry: BranchEntry,
    pub current: bool,
}

pub struct BranchMatch {
    pub row: usize,
    pub positions: Vec<usize>,
}

pub struct BranchState {
    rows: Vec<BranchRow>,
    pub query: String,
    pub matches: Vec<BranchMatch>,
    pub selected: usize,
    pub list_state: ListState,
}

impl BranchState {
    /// current: 非 detached 時の現在ブランチ名 (App.branch_status から)。detached や
    /// 取得不能なら None を渡す (current マークが一つも付かないだけで一覧自体は表示する)
    pub fn new(root: &Path, current: Option<&str>) -> Self {
        let rows = git::branches(root)
            .into_iter()
            .map(|entry| {
                let is_current = !entry.remote && current.is_some_and(|c| c == entry.name);
                BranchRow {
                    entry,
                    current: is_current,
                }
            })
            .collect();
        let mut state = Self {
            rows,
            query: String::new(),
            matches: Vec::new(),
            selected: 0,
            list_state: ListState::default(),
        };
        state.rescan();
        state
    }

    pub fn total(&self) -> usize {
        self.rows.len()
    }

    pub fn row(&self, idx: usize) -> Option<&BranchRow> {
        self.rows.get(idx)
    }

    pub fn selected_row(&self) -> Option<&BranchRow> {
        let m = self.matches.get(self.selected)?;
        self.row(m.row)
    }

    pub fn push_char(&mut self, c: char) {
        self.query.push(c);
        self.rescan();
    }

    pub fn backspace(&mut self) {
        self.query.pop();
        self.rescan();
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.matches.is_empty() {
            return;
        }
        let last = self.matches.len() as isize - 1;
        self.selected = (self.selected as isize + delta).clamp(0, last) as usize;
    }

    /// Ctrl+n の可否判定用。入力文字列がローカルブランチ名と完全一致する間は新規作成させない
    /// (同名で `git switch -c` すると既存ブランチと衝突してエラーになるだけなので、事前に弾く)
    pub fn matches_existing_local(&self) -> bool {
        self.rows
            .iter()
            .any(|r| !r.entry.remote && r.entry.name == self.query)
    }

    // クエリでスコアリング → ローカル/リモートで安定ソート (グループ内の順序はスコア順 or
    // クエリ空なら for-each-ref の committerdate 降順のまま) という2段階。
    // stable sort なので後段の並べ替えが前段の順序を壊さない
    fn rescan(&mut self) {
        let mut matches: Vec<BranchMatch> = if self.query.is_empty() {
            (0..self.rows.len())
                .map(|i| BranchMatch {
                    row: i,
                    positions: Vec::new(),
                })
                .collect()
        } else {
            let mut scored: Vec<(i64, BranchMatch)> = self
                .rows
                .iter()
                .enumerate()
                .filter_map(|(i, row)| {
                    let (score, positions) = fuzzy_match(&row.entry.name, &self.query)?;
                    Some((score, BranchMatch { row: i, positions }))
                })
                .collect();
            scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
            scored.into_iter().map(|(_, m)| m).collect()
        };
        matches.sort_by_key(|m| self.rows[m.row].entry.remote);
        self.matches = matches;
        self.selected = self.selected.min(self.matches.len().saturating_sub(1));
    }
}
