// バックグラウンドジョブの汎用基盤。std::thread::spawn + mpsc::channel でブロッキング処理
// (ネットワークを伴う git コマンド等) を投げっぱなしにし、結果は呼び出し側 (App::on_tick) が
// 既存の 100ms poll ループの中で try_recv して drain するだけにする。専用タイマーやブロッキング
// read を新設しないための唯一の入口。GIT リモート操作 (#27) 専用ではなく、将来の GitHub 連携
// (issues/PR の取得等) もここへ乗せる想定であえて git 非依存にしてある

use std::sync::mpsc::{self, Receiver};
use std::thread;

/// work を別スレッドで実行し、結果を受け取る Receiver を返す。呼び出し元 (App) が終了等で
/// Receiver を先に破棄しても `tx.send` は Err を返すだけなので無視してよい (issue の要求通り
/// panic させない)。ジョブを待たずに終了できるのもこの非同期さのおかげで、main::restore_terminal
/// を遅らせる要因にならない
pub fn spawn<T, F>(work: F) -> Receiver<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let result = work();
        let _ = tx.send(result);
    });
    rx
}
