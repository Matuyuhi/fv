use std::path::PathBuf;

use crate::component::branch::BranchState;
use crate::component::editor::EditState;
use crate::component::finder::Finder;
use crate::component::gitlane::GitState;

use super::commit::CommitDraft;

/// 入力を受け取るペイン。左ペイン (Tree) と右ペイン (Viewer) の 2 値だったところに、
/// VIEW レーンの左ペイン下半分に出るコミット一覧 (Log) が 3 つ目として加わる。
/// GIT の「ツリーを変更ファイルに絞り込む」や issues/PR の「一覧 + 詳細」は今まで通り
/// Tree/Viewer の意味を再利用するので、増えるのはこの 1 つだけで足りる
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Tree,
    /// コミット一覧ペイン (`L` で出している間だけ入れる)
    Log,
    Viewer,
}

// Search と Goto (:N 行ジャンプ) の入力を kind で分ける。Filter は issues/PR タブ (#33/#34) の
// 一覧絞り込み用で、Search と違い「常設のフィルタ状態を編集する」ものなので Esc の意味が違う
// (Search は cancel で全消去、Filter は編集前のクエリへ復元。issues::IssuesState 参照)
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    Search,
    Goto,
    Filter,
}

// 設定画面の行ラベル。行の並び・件数はこの配列が唯一の情報源で、
// keys.rs (選択移動・selected の意味) と shell/settings.rs (表示) の両方がここを参照する
pub const SETTINGS_ROWS: [&str; 7] = [
    "hidden files",
    "gitignored",
    "icons",
    "wrap (default)",
    "theme",
    "github tabs",
    "language",
];

#[derive(Default)]
pub struct SettingsState {
    pub selected: usize,
}

/// 持続する作業レーン。Shift+Tab で View → Edit → Git → View と循環する。
/// Edit / Git はそれぞれの状態を所有し「そのレーンにいるのに状態が無い」を型で排除する
/// (Finder と同じパターン)。オーバーレイ (Mode) を挟んでもレーンは保持されるので、
/// GIT でヘルプを開いて閉じても GIT に戻る。
///
/// コミット履歴はかつて Lane::Log として 4 つ目のレーンだったが、「ファイルを読みながら
/// 履歴も見る」が本来の使い方で、レーンを 1 つ消費して画面を丸ごと差し替えるのは強すぎた。
/// 今は VIEW の左ペイン下半分に出る一覧 (App::log) になっていて、レーンではなく `L` の
/// トグルで on/off する
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
    // Ctrl+f ワークスペース横断検索のオーバーレイ。状態は Finder と違い Mode の中ではなく
    // App.grep に常駐させる — 背景の走査が閉じた後も続き、開き直した時に前回の結果が
    // そのまま見える (大きい repo で同じクエリを何度も歩き直さない) ようにするため
    Grep,
    // キーバインド一覧のオーバーレイ。全レーン分を 1 枚に並べると端末の高さに収まらず、
    // 後半のセクション (Git/Log/Edit 以降) が黙って切れて「ヘルプに載っていない」ように
    // 見えてしまうため、読み位置だけを状態として持つ。専用の状態型を作るほどではないので
    // Mode::Commit と同じくフィールドを直接持つ (= shell 側の画面)
    Help {
        scroll: usize,
    },
    // 設定画面のオーバーレイ (s キー)
    Settings(SettingsState),
    // 破壊的・書き込み系操作の確認オーバーレイ。Lane と直交する (GIT で出しても EDIT で出しても
    // 同じ挙動)。y/Enter でのみ action を実行し、それ以外の全キーは中止として扱う。
    // #23 (stage/unstage) は非破壊的なのでここを経由させない
    Confirm {
        prompt: String,
        action: ConfirmAction,
    },
    // コミットメッセージ入力オーバーレイ (`c`/`C`)。Search/Goto の 1 行入力用 Input では
    // 複数行編集を表現できないため独立させた。件名と本文は別の入力欄として CommitDraft が持つ。
    // error は pre-commit hook 失敗時の stderr 要約 — Esc/破棄せず同じオーバーレイに留めて
    // 見せるため Mode 自体に持たせる (App.notice だとオーバーレイを閉じた後の表示になってしまう)
    Commit {
        draft: CommitDraft,
        amend: bool,
        error: Option<String>,
    },
    // ブランチ一覧オーバーレイ (`b`)。Lane と直交する独立オーバーレイなので Finder と同じ
    // 位置付けで、状態 (絞り込み候補・選択位置) は BranchState (component/branch/mod.rs) に持たせる
    Branch(BranchState),
}

/// Mode::Confirm が実行する操作。クロージャは App を借りたまま呼べず持たせられないため
/// enum にする。書き込み系の子 issue が実装されるたびにここへ variant を足していく想定。
/// #23 (stage/unstage) は非破壊的操作なので Confirm を経由させない
pub enum ConfirmAction {
    // amend は履歴を書き換える (push 済みの可能性がある) ので確認を必須にする。
    // 通常コミットは確認なしで直接実行する (issue #24 の要求通り)
    Amend {
        message: String,
    },
    /// 選択ファイル/ディレクトリの変更破棄 (#25)。is_dir は tracked/untracked の扱いを
    /// 分けるために確認時点の Row から引き継ぐ (実行時に fs へ問い合わせ直さない)
    Discard {
        path: PathBuf,
        is_dir: bool,
    },
    /// `git stash push -u` (#25)。untracked も含めて退避する
    StashPush,
    /// `git stash pop` (#25)。コンフリクト時は stash entry を残したまま notice にエラーを出す
    StashPop,
    /// `P`: push (#27)。fetch/pull と違いリモートの履歴・ブランチ構成を変えるので確認必須にする
    Push,
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
    pub const LABELS: [&'static str; 3] = ["Viewer", "Issues", "Pull Requests"];

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
