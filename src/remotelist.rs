//! issues (#33) / pull requests (#34) タブが共有する一覧・詳細キャッシュの土台。
//! 「gh の一覧取得 → フィルタ (query + state) → 選択 → 詳細を番号ごとに非同期キャッシュ」
//! という形がどちらも完全に同じなので、絞り込みの実アルゴリズム (`filter_rows`) と
//! 詳細の非同期キャッシュ (`DetailSlot`) をここに 1 度だけ実装して両タブから使う。
//! state 絞り込みのカーディナリティ (issues は open/closed/all、PR は
//! open/closed/merged/all) と一覧の行の型 (issues は RemoteItem そのもの、PR は
//! headRefName/isDraft を足した PrRow) は呼び出し側ごとに違うため、ここには持ち込まず
//! `ListRow` トレイト (title/state を引けること) 越しに扱う。

use std::collections::HashMap;
use std::sync::mpsc::Receiver;

use crate::finder::fuzzy_match;
use crate::github::RemoteItem;

/// 一覧のフィルタ結果 1 行。row は絞り込み対象スライスの index、positions はタイトル内で
/// マッチした char インデックス (branch_panel::highlight_name と同じハイライト用途)
pub struct ListMatch {
    pub row: usize,
    pub positions: Vec<usize>,
}

/// フィルタ・スコアリングが必要とする最小限のアクセサ。issues は RemoteItem をそのまま、
/// PR は RemoteItem を包んだ PrRow がこれを実装する (RemoteItem 自体を汚さないため)
pub trait ListRow {
    fn title(&self) -> &str;
    fn state(&self) -> &str;
}

impl ListRow for RemoteItem {
    fn title(&self) -> &str {
        &self.title
    }
    fn state(&self) -> &str {
        &self.state
    }
}

/// issuesview::IssuesState::rescan / prsview::PrsState::rescan が共有するフィルタ + スコアリング。
/// クエリが空ならスコアリングをせず gh の並び順 (updated 降順) をそのまま保ち、クエリがあれば
/// fuzzy_match でスコアリングしてから降順に並べ替える (branch.rs::BranchState と対称の 2 段階)
pub fn filter_rows<R: ListRow>(
    rows: &[R],
    query: &str,
    accepts: impl Fn(&str) -> bool,
) -> Vec<ListMatch> {
    if query.is_empty() {
        rows.iter()
            .enumerate()
            .filter(|(_, r)| accepts(r.state()))
            .map(|(i, _)| ListMatch {
                row: i,
                positions: Vec::new(),
            })
            .collect()
    } else {
        let mut scored: Vec<(i64, ListMatch)> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| accepts(r.state()))
            .filter_map(|(i, r)| {
                let (score, positions) = fuzzy_match(r.title(), query)?;
                Some((score, ListMatch { row: i, positions }))
            })
            .collect();
        scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
        scored.into_iter().map(|(_, m)| m).collect()
    }
}

/// 番号 (issue/PR number) ごとに非同期取得・キャッシュする詳細スロット。issues の詳細
/// (1 種類) と PR の説明/diff/CI (3 種類、prsview.rs) がどちらも「取得中/キャッシュ済み/
/// エラー を番号で持つ」という同じ形なので、表示用に組み立て済みのデータ型 T だけ差し替えて
/// 共有する (T は issues/PR の説明なら Vec<Line<'static>>、PR の diff なら専用の構造体)
pub struct DetailSlot<T> {
    cache: HashMap<u64, T>,
    errors: HashMap<u64, String>,
    loading: Option<u64>,
    rx: Option<Receiver<(u64, Result<T, String>)>>,
}

impl<T> DetailSlot<T> {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
            errors: HashMap::new(),
            loading: None,
            rx: None,
        }
    }

    /// 選択された番号の表示を要求する。既にキャッシュ済み・取得中なら false を返し、
    /// 呼び出し側は job を起動しない (Enter 連打での二重起動防止)
    pub fn request(&mut self, number: u64) -> bool {
        if self.cache.contains_key(&number) || self.loading == Some(number) {
            return false;
        }
        self.errors.remove(&number);
        self.loading = Some(number);
        true
    }

    pub fn begin_fetch(&mut self, rx: Receiver<(u64, Result<T, String>)>) {
        self.rx = Some(rx);
    }

    pub fn loading(&self, number: u64) -> bool {
        self.loading == Some(number)
    }

    pub fn error(&self, number: u64) -> Option<&str> {
        self.errors.get(&number).map(String::as_str)
    }

    pub fn get(&self, number: u64) -> Option<&T> {
        self.cache.get(&number)
    }

    /// on_tick から drain する。新しく結果が届いた番号を返す (呼び出し側が「今開いているのと
    /// 同じ番号か」を見て viewport リセット等の追加処理をするための戻り値)
    pub fn poll(&mut self) -> Option<u64> {
        let Some(rx) = &self.rx else {
            return None;
        };
        let Ok((number, result)) = rx.try_recv() else {
            return None;
        };
        self.rx = None;
        if self.loading == Some(number) {
            self.loading = None;
        }
        match result {
            Ok(value) => {
                self.cache.insert(number, value);
            }
            Err(message) => {
                self.errors.insert(number, message);
            }
        }
        Some(number)
    }
}

impl<T> Default for DetailSlot<T> {
    fn default() -> Self {
        Self::new()
    }
}
