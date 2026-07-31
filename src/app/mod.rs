mod keys;
mod mode;
mod mouse;

pub use mode::{Focus, InputKind, Lane, Mode, SETTINGS_ROWS, SettingsState, Workspace};

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ratatui::layout::Rect;

use crate::config::Config;
use crate::editor::EditState;
use crate::git::{self, GitStatus};
use crate::github;
use crate::gitview::GitState;
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
    /// トップレベルのタブ (Lane/Mode に続く3本目の軸)。GitHub モードが無効/使えない間は
    /// 常に Viewer 固定 (workspace_available が false の間、切替キーは全て no-op にする)
    pub workspace: Workspace,
    pub tree: Tree,
    pub viewer: Viewer,
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
    /// タブバーの各タブの矩形 (workspace_available が false の間は全て空)。
    /// ui::tab_bar が毎フレーム書き戻し、mouse.rs のクリック判定が読む
    pub tab_areas: [Rect; Workspace::LABELS.len()],
    /// スプリッタのドラッグ中はその掴んだ桁のオフセット (0 or 1) を保持する。
    /// 掴み位置を覚えることで Down の瞬間に境界が 1 桁飛ばない
    dragging_split: Option<u16>,
    // 左ペインが画面幅に占める割合 (config に永続化)
    split_ratio: f32,
    watcher: Option<FsWatcher>,
    last_rescan: Instant,
    rescan_pending: bool,
    /// GitHub モードを使いたいかどうか (起動オプション or 設定トグル)。使えるかどうかは別
    /// (github_available)。3 経路 (--github / 設定トグル / config ファイル) が結局この
    /// フラグ 1 つに集約される
    pub github_enabled: bool,
    /// config ファイルへ永続化する値。cli の --github はここへは影響しない
    /// (「その起動でだけ有効」を current_config 側で守るため、実行時フラグと分けて持つ)
    github_persisted: bool,
    /// gh の有無・認証・GitHub リモートかどうかの判定結果。起動時 (または初回有効化時) に
    /// 1 度だけ github::check_available を呼んで確定させ、以後は描画のたびに再判定しない
    pub github_available: bool,
    github_checked: bool,
    /// 直近のグローバル通知 (GitHub 有効化不可の理由など)。次のキー入力で消えるトースト表示
    pub notice: Option<String>,
}

impl App {
    /// github_cli は `--github` の指定。その起動限りの有効化で config には書かない
    /// (config.github との合成は github_enabled の初期値としてのみ行う)
    pub fn new(root: PathBuf, config: Config, github_cli: bool) -> Self {
        let tree = Tree::new(&root, config.show_hidden);
        // 監視の初期化に失敗しても (権限等) 監視なしで起動を続ける
        let watcher = FsWatcher::new(&root, config.show_hidden);
        let git = git::file_statuses(&root);
        let mut viewer = Viewer::new();
        viewer.viewport.wrap = config.wrap_default;
        // 設定ファイルのテーマ名が壊れていても set_theme が false を返すだけで、
        // Viewer::new() が入れた既定テーマのまま起動を続ける (パニックしない)
        viewer.set_theme(&config.theme);
        let github_enabled = github_cli || config.github;
        let mut app = Self {
            root,
            focus: Focus::Tree,
            lane: Lane::View,
            mode: Mode::Normal,
            workspace: Workspace::Viewer,
            tree,
            viewer,
            git,
            icons: config.icons,
            should_quit: false,
            pending_g: false,
            tree_area: Rect::default(),
            viewer_area: Rect::default(),
            splitter_area: Rect::default(),
            tab_areas: Default::default(),
            dragging_split: None,
            split_ratio: config.split_ratio,
            watcher,
            last_rescan: Instant::now(),
            rescan_pending: false,
            github_enabled,
            github_persisted: config.github,
            github_available: false,
            github_checked: false,
            notice: None,
        };
        // 判定は起動時に 1 回だけ。無効なら gh を叩くコスト自体を払わない
        if app.github_enabled {
            app.ensure_github_checked();
        }
        app
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
        let Some(watcher) = &self.watcher else {
            return;
        };
        let changed = watcher.drain();
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
        self.tree.rescan(&self.root);
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
        // Issues/PR タブに Lane の概念は無いので、居る間は循環を無効にする
        // (ステータスバー側もこれに合わせてセグメントを暗くする)
        if !matches!(self.workspace, Workspace::Viewer) {
            return;
        }
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

    /// GitHub モードのタブバーを実際に出してよいか。有効化されていて (github_enabled) かつ
    /// 使える環境 (github_available) の両方が揃って初めて true。タブバーの描画・Ctrl+t /
    /// Alt+1..3 のキー・クリック判定が全てこの 1 箇所を参照する
    pub fn workspace_available(&self) -> bool {
        self.github_enabled && self.github_available
    }

    // gh の有無・認証・GitHub リモートかどうかを判定する。1 度確定したら github_checked で
    // 以後は呼び出しても何もしない (起動時 1 回・初回有効化時 1 回だけに絞るため)
    fn ensure_github_checked(&mut self) {
        if self.github_checked {
            return;
        }
        self.github_checked = true;
        match github::check_available(&self.root) {
            Ok(()) => self.github_available = true,
            Err(reason) => {
                self.github_available = false;
                self.notice = Some(reason);
            }
        }
    }

    /// 設定オーバーレイの "github tabs" トグル。cli 由来の有効化と違い、ここでの変更は
    /// そのまま config に永続化する (persisted と実行時フラグを同じ値に揃えることで、
    /// 一度トグルすれば --github の有無に関わらずその通りの状態になる)
    pub(super) fn toggle_github(&mut self) {
        self.github_enabled = !self.github_enabled;
        self.github_persisted = self.github_enabled;
        self.persist_config();
        if self.github_enabled {
            self.ensure_github_checked();
        }
        if !self.workspace_available() {
            self.workspace = Workspace::Viewer;
        }
    }

    /// Ctrl+t: 次のタブへ循環する。使えない (workspace_available が false) 間は無効化時と
    /// 同じ経路を通るだけの no-op になる
    pub(super) fn cycle_workspace(&mut self) {
        if !self.workspace_available() {
            return;
        }
        self.workspace = Workspace::from_index((self.workspace.index() + 1) % 3);
    }

    /// Alt+1..3・タブクリック共通の直接指定。使えない間は no-op
    pub(super) fn set_workspace(&mut self, workspace: Workspace) {
        if !self.workspace_available() {
            return;
        }
        self.workspace = workspace;
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
        let show_hidden = self.tree.toggle_hidden(&self.root);
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
            // github_enabled ではなく github_persisted を使う。cli 由来の一時的な有効化が
            // 他の設定操作 (ペイン幅ドラッグ等) の persist_config に巻き込まれて書き込まれるのを防ぐ
            github: self.github_persisted,
        }
    }

    // 保存失敗 (権限なし等) はここで握り潰す。読み取り専用ビューアの付随機能が
    // ファイル書き込み失敗でクラッシュ・エラー表示をする理由はない
    fn persist_config(&self) {
        let _ = self.current_config().save();
    }
}

// 最小幅を満たせない極端に狭い端末では下限を諦めて半分ずつにする
// (clamp の lo > hi でパニックさせない)
fn clamp_tree_width(width: u16, total: u16) -> u16 {
    if total < MIN_TREE_WIDTH + MIN_VIEWER_WIDTH {
        return total / 2;
    }
    width.clamp(MIN_TREE_WIDTH, total - MIN_VIEWER_WIDTH)
}
