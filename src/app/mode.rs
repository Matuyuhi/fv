use crate::editor::EditState;
use crate::finder::Finder;
use crate::gitview::GitState;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Tree,
    Viewer,
}

// Search と Goto (:N 行ジャンプ) の入力を kind で分ける
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    Search,
    Goto,
}

// 設定画面の行ラベル。行の並び・件数はこの配列が唯一の情報源で、
// keys.rs (選択移動・selected の意味) と ui/settings_panel.rs (表示) の両方がここを参照する
pub const SETTINGS_ROWS: [&str; 4] = ["hidden files", "icons", "wrap (default)", "theme"];

#[derive(Default)]
pub struct SettingsState {
    pub selected: usize,
}

/// 持続する作業レーン。Shift+Tab で View → Edit → Git → View と循環する。
/// Edit / Git はそれぞれの状態を所有し「そのレーンにいるのに状態が無い」を型で排除する
/// (Finder と同じパターン)。オーバーレイ (Mode) を挟んでもレーンは保持されるので、
/// GIT でヘルプを開いて閉じても GIT に戻る
pub enum Lane {
    View,
    Edit(EditState),
    Git(GitState),
}

impl Lane {
    /// ステータスバーのセグメント表示。並び順は Shift+Tab の循環順と同じ
    pub const LABELS: [&'static str; 3] = ["VIEW", "EDIT", "GIT"];

    pub fn index(&self) -> usize {
        match self {
            Lane::View => 0,
            Lane::Edit(_) => 1,
            Lane::Git(_) => 2,
        }
    }
}

/// レーンの上に重なる一時状態。閉じると Normal に戻り、レーンはそのまま残る。
/// Shift+Tab の循環対象には含めない (キーの意味が入力欄と衝突するため)
pub enum Mode {
    Normal,
    Input { kind: InputKind, buffer: String },
    // Ctrl+p ファジーファインダー。Input に押し込むと Search/Goto と挙動が絡み合うため独立させる
    Finder(Finder),
    // キーバインド一覧のオーバーレイ。状態を持たないので unit variant で十分
    Help,
    // 設定画面のオーバーレイ (s キー)
    Settings(SettingsState),
}
