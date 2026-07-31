mod keys;
mod mode;
mod mouse;

pub use mode::{Focus, InputKind, Lane, Mode, SETTINGS_ROWS, SettingsState};

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ratatui::layout::Rect;

use crate::config::Config;
use crate::editor::EditState;
use crate::git::{self, GitStatus};
use crate::gitview::GitState;
use crate::index::FileIndex;
use crate::tree::Tree;
use crate::viewer::{self, Viewer};
use crate::watch::FsWatcher;

// イベント嵐 (git checkout やビルド等) でツリーを毎回フル再走査しないための間引き間隔
const RESCAN_DEBOUNCE: Duration = Duration::from_millis(500);

// ペイン分割の下限幅 (枠線 2 桁を含む)。左はツリーの階層インデントが、
// 右は gutter + 数十桁のコードが最低限読める幅
const MIN_TREE_WIDTH: u16 = 12;
const MIN_VIEWER_WIDTH: u16 = 24;

pub struct App {
    pub root: PathBuf,
    pub focus: Focus,
    /// 持続する作業レーン (VIEW/EDIT/GIT)。Shift+Tab で循環する
    pub lane: Lane,
    /// レーンの上に重なる一時オーバーレイ。閉じてもレーンは変わらない
    pub mode: Mode,
    pub tree: Tree,
    pub viewer: Viewer,
    /// Finder の候補。ツリーが遅延走査になったぶん、全ファイル一覧は別に持つ
    pub file_index: FileIndex,
    // git repo でない / git 未インストールなら None のままで通常表示にフォールバックする
    pub git: Option<GitStatus>,
    // Nerd Font アイコン表示。起動時に確定し実行中は変わらない (判定は main 側)
    pub icons: bool,
    pub should_quit: bool,
    // g 待ち状態。Mode を増やすほどのものではないので App の小さなフラグで持つ
    pub pending_g: bool,
    // マウスのヒットテスト用。ui::draw が毎フレーム書き戻す (viewport の実測値と同じパターン)
    pub tree_area: Rect,
    pub viewer_area: Rect,
    // 左右ペインの境界 (両ペインの枠線 2 桁)。ドラッグの掴み判定用に ui::draw が書き戻す
    pub splitter_area: Rect,
    /// スプリッタのドラッグ中はその掴んだ桁のオフセット (0 or 1) を保持する。
    /// 掴み位置を覚えることで Down の瞬間に境界が 1 桁飛ばない
    dragging_split: Option<u16>,
    // 左ペインが画面幅に占める割合 (config に永続化)
    split_ratio: f32,
    watcher: FsWatcher,
    last_rescan: Instant,
    rescan_pending: bool,
}

impl App {
    pub fn new(root: PathBuf, config: Config) -> Self {
        let tree = Tree::new(&root, config.show_hidden);
        // 再帰監視の登録は別スレッドで進む (起動を待たせない)。失敗しても
        // 監視なしで動き続ける
        let watcher = FsWatcher::new(&root, config.show_hidden);
        let git = git::file_statuses(&root);
        let mut viewer = Viewer::new();
        viewer.viewport.wrap = config.wrap_default;
        // 設定ファイルのテーマ名が壊れていても set_theme が false を返すだけで、
        // Viewer::new() が入れた既定テーマのまま起動を続ける (パニックしない)
        viewer.set_theme(&config.theme);
        let file_index = FileIndex::new(root.clone(), config.show_hidden);
        Self {
            root,
            focus: Focus::Tree,
            lane: Lane::View,
            mode: Mode::Normal,
            tree,
            viewer,
            file_index,
            git,
            icons: config.icons,
            should_quit: false,
            pending_g: false,
            tree_area: Rect::default(),
            viewer_area: Rect::default(),
            splitter_area: Rect::default(),
            dragging_split: None,
            split_ratio: config.split_ratio,
            watcher,
            last_rescan: Instant::now(),
            rescan_pending: false,
        }
    }

    /// 保存された割合から左ペインの実桁数を求める。ui::draw と
    /// ドラッグ時の clamp が同じ定義を通るよう、換算はここ 1 箇所に閉じる
    pub fn tree_width(&self, total: u16) -> u16 {
        clamp_tree_width((self.split_ratio * total as f32).round() as u16, total)
    }

    /// ドラッグ中: 掴んだ桁が column に来るよう割合を更新する。
    /// 桁数で clamp してから割合に戻すので、下限幅に張り付いた後も割合が暴れない
    pub(super) fn set_split_at(&mut self, column: u16, grab: u16) {
        let total = self.tree_area.width + self.viewer_area.width;
        if total == 0 {
            return;
        }
        // 境界の桁 = 左ペインの右枠線なので、そこまでの桁数がそのまま左ペイン幅になる
        let target = column
            .saturating_sub(grab)
            .saturating_sub(self.tree_area.x)
            .saturating_add(1);
        self.split_ratio = clamp_tree_width(target, total) as f32 / total as f32;
    }

    /// watcher に溜まったファイル変更を取り込む。キー入力の有無に関わらず、
    /// イベントループの毎 tick (poll タイムアウト時も含む) で呼ばれる。
    pub fn on_tick(&mut self) {
        // 背景走査が終わったら、開いたままの Finder の候補も差し替える
        // (走査中に開いた場合は読み込み済み分だけの暫定候補になっているため)
        if self.file_index.poll()
            && let Mode::Finder(finder) = &mut self.mode
            && let Some(files) = self.file_index.files()
        {
            finder.set_candidates(to_candidates(files));
        }

        let changed = self.watcher.drain();
        let open_path = self.viewer.current.as_ref().map(|open| open.path.clone());

        for path in &changed {
            if open_path.as_deref() == Some(path.as_path()) {
                self.viewer.reload(path);
            } else {
                self.rescan_pending = true;
            }
        }

        // GIT レーンでは絞り込みと diff も古くなるので、専用タイマーを作らず
        // 同じ 500ms デバウンス (rescan) に相乗りさせる
        if !changed.is_empty() && matches!(self.lane, Lane::Git(_)) {
            self.rescan_pending = true;
        }

        if self.rescan_pending && self.last_rescan.elapsed() >= RESCAN_DEBOUNCE {
            self.rescan();
            self.last_rescan = Instant::now();
            self.rescan_pending = false;
        }
    }

    /// ツリーと git status をまとめて再取得する。FS 監視の間引き後と、
    /// 手動再走査 (r キー) の両方から呼ばれる共通処理。
    fn rescan(&mut self) {
        self.tree.rescan();
        // ツリーに現れない (未展開の) 変更も候補一覧には効くので、次に Finder を
        // 開くときに歩き直させる。ここで走査を起こすと保存のたびに全走査になる
        self.file_index.invalidate();
        self.git = git::file_statuses(&self.root);
        if !matches!(self.lane, Lane::Git(_)) {
            return;
        }
        // 滞在中に変更が全部無くなった場合 (別端末での commit / stash 等)。
        // 空のツリーに取り残さず VIEW へ戻す
        if !self.git_available() {
            self.tree.set_filter(None);
            self.lane = Lane::View;
            return;
        }
        // 絞り込みも表示中 diff も新しい git status に追従させる
        self.tree.set_filter(Some(self.changed_paths()));
        let root = self.root.clone();
        if let Lane::Git(git) = &mut self.lane {
            git.refresh(&root);
        }
    }

    /// Shift+Tab: VIEW → EDIT → GIT → VIEW と循環する。入れないレーン
    /// (非テキストの EDIT、非 git repo の GIT) は飛ばし、一周して戻れなければ現状維持。
    pub(super) fn cycle_lane(&mut self) {
        // 未保存の編集を Shift+Tab で取りこぼさない (Esc の discard 確認と同じ理由)
        if let Lane::Edit(state) = &mut self.lane
            && state.buffer.dirty()
        {
            state.notice = Some("未保存の変更があります (Ctrl+s: 保存 / Esc: 破棄)".to_string());
            return;
        }
        self.pending_g = false;
        let mut index = self.lane.index();
        for _ in 0..Lane::LABELS.len() {
            index = (index + 1) % Lane::LABELS.len();
            if self.enter_lane(index) {
                return;
            }
        }
    }

    // Lane::LABELS の index に対応するレーンへ入る。入れなければ false を返して呼び出し側で次へ送る
    fn enter_lane(&mut self, index: usize) -> bool {
        let entered = match index {
            1 => self.enter_edit(),
            2 => self.enter_git(),
            _ => {
                self.lane = Lane::View;
                true
            }
        };
        // GIT を離れたらツリーの絞り込みを解除する。実 expanded フラグは触っていないので
        // 元の展開状態がそのまま戻る
        if entered && !matches!(self.lane, Lane::Git(_)) {
            self.tree.set_filter(None);
        }
        entered
    }

    /// EDIT レーンへ入る。非テキスト・巨大ファイル・非 UTF-8 は false (Shift+Tab では飛ばされ、
    /// e キーからは no-op になる)
    pub(super) fn enter_edit(&mut self) -> bool {
        if !self.viewer.is_text() {
            return false;
        }
        let Some(open) = &self.viewer.current else {
            return false;
        };
        let Some(state) = EditState::open(
            &open.path.clone(),
            &self.viewer.highlighter,
            self.viewer.viewport.scroll,
            &self.root,
        ) else {
            return false;
        };
        self.lane = Lane::Edit(state);
        true
    }

    /// GIT レーンに入れるか。非 git repo (git 未インストール含む) と、変更が 1 件も無いときは
    /// 入れない (空のツリーと空の diff を見せても意味がない)。ステータスバーの活性表示も
    /// 同じ判定を参照するので、見た目と実際の可否がずれない
    pub fn git_available(&self) -> bool {
        self.git
            .as_ref()
            .is_some_and(|status| !status.files.is_empty())
    }

    fn enter_git(&mut self) -> bool {
        if !self.git_available() {
            return false;
        }
        self.tree.set_filter(Some(self.changed_paths()));
        let mut git = GitState::new(self.viewer.viewport.wrap);
        if let Some(path) = self.tree.selected_or_first_file() {
            git.open(&self.root, &path);
        }
        self.lane = Lane::Git(git);
        // 入った直後は「どのファイルを見るか」の選択が主操作なのでツリー側にフォーカスを寄せる
        self.focus = Focus::Tree;
        true
    }

    /// ツリー・ファインダーからの「開く」の振り分け。VIEW/EDIT は viewer、GIT は diff を差し替える
    pub(super) fn open_selected(&mut self, path: &Path) {
        match &mut self.lane {
            Lane::Git(git) => git.open(&self.root, path),
            _ => self.viewer.open(path, &self.root),
        }
    }

    // 変更ファイルとその祖先ディレクトリ。どちらも git status 取得時に組んだ集合を使い回すだけで、
    // ツリーの再走査は要らない
    fn changed_paths(&self) -> HashSet<PathBuf> {
        match &self.git {
            Some(status) => status
                .files
                .keys()
                .cloned()
                .chain(status.changed_dirs.iter().cloned())
                .collect(),
            None => HashSet::new(),
        }
    }

    /// bracketed paste (main のイベントループから)。編集バッファへは複数行のまま、
    /// Search/Goto/Finder の 1 行入力へは制御文字を落として流す
    /// (paste 有効化前は生キー入力として届いていた挙動の維持)
    pub fn on_paste(&mut self, text: &str) {
        if let Lane::Edit(state) = &mut self.lane {
            state.paste(text, &self.viewer.highlighter, &mut self.viewer.viewport);
            return;
        }
        match &mut self.mode {
            Mode::Input { kind, buffer } => {
                let kind = *kind;
                for c in text.chars().filter(|c| !c.is_control()) {
                    // キー入力側 (on_input_key) と同じ Goto の数字ガードを通す
                    if kind == InputKind::Goto && !c.is_ascii_digit() {
                        continue;
                    }
                    buffer.push(c);
                }
                self.live_update_input(kind);
            }
            Mode::Finder(finder) => {
                for c in text.chars().filter(|c| !c.is_control()) {
                    finder.push_char(c);
                }
            }
            _ => {}
        }
    }

    pub fn toggle_hidden(&mut self) {
        let show_hidden = self.tree.toggle_hidden();
        self.file_index.set_show_hidden(show_hidden);
        // 既存 watcher のキューには切替前のフィルタ結果が残るため、監視も作り直して揃える。
        self.watcher = FsWatcher::new(&self.root, show_hidden);
        self.last_rescan = Instant::now();
        self.rescan_pending = false;
        self.persist_config();
    }

    pub fn toggle_icons(&mut self) {
        self.icons = !self.icons;
        self.persist_config();
    }

    pub fn toggle_wrap(&mut self) {
        self.viewer.viewport.toggle_wrap();
        self.persist_config();
    }

    /// delta の符号方向に THEME_NAMES を巡回する (設定画面の h/l 用)
    pub fn cycle_theme(&mut self, delta: isize) {
        let names = viewer::THEME_NAMES;
        let current = self.viewer.theme_name();
        let idx = names.iter().position(|n| *n == current).unwrap_or(0) as isize;
        let len = names.len() as isize;
        let next = (idx + delta).rem_euclid(len) as usize;
        self.viewer.set_theme(names[next]);
        self.persist_config();
    }

    fn current_config(&self) -> Config {
        Config {
            show_hidden: self.tree.show_hidden(),
            icons: self.icons,
            wrap_default: self.viewer.viewport.wrap,
            theme: self.viewer.theme_name().to_string(),
            split_ratio: self.split_ratio,
        }
    }

    // 保存失敗 (権限なし等) はここで握り潰す。読み取り専用ビューアの付随機能が
    // ファイル書き込み失敗でクラッシュ・エラー表示をする理由はない
    fn persist_config(&self) {
        let _ = self.current_config().save();
    }
}

// Finder の候補は相対パス文字列。FileIndex とツリー (走査完了前の代用) の
// どちらから来ても同じ形に揃える
pub(super) fn to_candidates(files: &[PathBuf]) -> Vec<String> {
    files
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect()
}

// 最小幅を満たせない極端に狭い端末では下限を諦めて半分ずつにする
// (clamp の lo > hi でパニックさせない)
fn clamp_tree_width(width: u16, total: u16) -> u16 {
    if total < MIN_TREE_WIDTH + MIN_VIEWER_WIDTH {
        return total / 2;
    }
    width.clamp(MIN_TREE_WIDTH, total - MIN_VIEWER_WIDTH)
}
