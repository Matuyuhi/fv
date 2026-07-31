// GitHub モードが使える環境かどうかの判定。呼ぶのは App::new / toggle_github からの
// 1 回きりで、描画のたびには叩かない (CLAUDE.md の GIT レーンと同じ「重い処理はイベントループを
// ブロックしない」方針とは別に、そもそも起動時 1 回に絞ることでブロック自体を避けている)。
use std::path::Path;
use std::process::Command;

/// 使えれば Ok(())、使えなければ理由 (notice にそのまま出す文言) を返す
pub fn check_available(root: &Path) -> Result<(), String> {
    match Command::new("gh")
        .args(["auth", "status"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .current_dir(root)
        .output()
    {
        Ok(output) if output.status.success() => {}
        Ok(_) => {
            return Err(
                "GitHub モードを有効化できません: gh が未認証です (gh auth login)".to_string(),
            );
        }
        Err(_) => {
            return Err("GitHub モードを有効化できません: gh コマンドが見つかりません".to_string());
        }
    }
    let remote = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .current_dir(root)
        .output();
    let is_github_remote = match remote {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).contains("github.com")
        }
        _ => false,
    };
    if !is_github_remote {
        return Err(
            "GitHub モードを有効化できません: origin が GitHub リポジトリではありません"
                .to_string(),
        );
    }
    Ok(())
}
