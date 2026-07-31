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
pub const SETTINGS_ROWS: [&str; 5] = [
    "hidden files",
    "icons",
    "wrap (default)",
    "theme",
    "github tabs",
];

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
    Input {
        kind: InputKind,
        buffer: String,
    },
    // Ctrl+p ファジーファインダー。Input に押し込むと Search/Goto と挙動が絡み合うため独立させる
    Finder(Finder),
    // キーバインド一覧のオーバーレイ。状態を持たないので unit variant で十分
    Help,
    // 設定画面のオーバーレイ (s キー)
    Settings(SettingsState),
    // 破壊的・書き込み系操作の確認オーバーレイ。Lane と直交する (GIT で出しても EDIT で出しても
    // 同じ挙動)。y/Enter でのみ action を実行し、それ以外の全キーは中止として扱う
    Confirm {
        prompt: String,
        action: ConfirmAction,
    },
}

/// Mode::Confirm が実行する操作。クロージャは App を借りたまま呼べず持たせられないため
/// enum にする。書き込み系の子 issue (stage / commit / discard / stash 等) が実装されるたびに
/// ここへ variant を足していく想定
pub enum ConfirmAction {
    /// 書き込み経路 (run_git_write) と確認オーバーレイの動作確認用。実際の書き込み機能
    /// (#23 以降) が入り次第、この variant は役目を終える
    DemoGitRefresh,
}

/// トップレベルのタブ ("Workspace")。Lane / Mode に続く 3 本目の軸で、GitHub モード
/// (#32) 有効時だけヘッダに 1 行のタブバーとして現れる。Viewer が既存アプリ全体
/// (Lane 3 種 + ツリー + オーバーレイ) にあたり、Issues / PullRequests は「ローカルの
/// ファイル」という文脈を共有しないリモートのデータなので Lane には混ぜない。
/// #33 / #34 で中身が入るまでは状態を持たない unit variant のままで良い
/// (Lane::View が状態を持たないのと同じ理由)
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Workspace {
    Viewer,
    Issues,
    PullRequests,
}

impl Workspace {
    /// タブバー・ステータスバーの表示ラベル。並び順は Ctrl+t の循環順・Alt+1..3 の対応と同じ
    pub const LABELS: [&'static str; 3] = ["viewer", "issues", "pull requests"];

    pub fn index(self) -> usize {
        match self {
            Workspace::Viewer => 0,
            Workspace::Issues => 1,
            Workspace::PullRequests => 2,
        }
    }

    // Alt+1..3 とタブクリックの両方が同じ変換を通るための唯一の入口
    pub fn from_index(index: usize) -> Self {
        match index {
            1 => Workspace::Issues,
            2 => Workspace::PullRequests,
            _ => Workspace::Viewer,
        }
    }
}
