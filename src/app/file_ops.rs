//! ツリー上のファイル操作 (新規作成・ディレクトリ作成・リネーム・削除・パスのコピー)。
//! キーの割り当ては keys.rs (Focus::Tree の分岐) に置き、ここは操作の中身だけを持つ。
//! git を経由せず std::fs で直接書くので、tracked/untracked を問わず同じ挙動になる
//! (git 側の追従は他の書き込み系操作と同じく rescan_now に相乗りさせる)。

use std::path::{Component, Path, PathBuf};

use crate::clipboard;
use crate::lang::t;

use super::{App, ConfirmAction, InputKind, Mode};

/// Mode::Input で入力中のファイル操作の対象。InputKind は Copy の識別子だけなので、
/// パスを要する部分はこちらで持つ (Input を開いた時に立て、閉じた時に落とす)
pub(super) enum FileOp {
    /// 新規作成の親ディレクトリ。選択行がディレクトリならそれ、ファイルならその親
    Create { dir: PathBuf },
    /// リネーム元。入力欄には末尾の名前だけを出し、親は据え置く
    Rename { from: PathBuf },
}

impl App {
    /// `n` (ファイル) / `N` (ディレクトリ)。選択行が無い (空のツリー) なら root 直下に作る
    pub(super) fn open_new_entry(&mut self, is_dir: bool) {
        let dir = match self.tree.visible.get(self.tree.selected) {
            Some(row) if row.is_dir => row.path.clone(),
            Some(row) => row
                .path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| self.root.clone()),
            None => self.root.clone(),
        };
        self.pending_g = false;
        self.file_op = Some(FileOp::Create { dir });
        self.mode = Mode::Input {
            kind: if is_dir {
                InputKind::NewDir
            } else {
                InputKind::NewFile
            },
            buffer: String::new(),
        };
    }

    /// `R`。入力欄は現在の名前でプリフィルする (1 文字直したいだけの時に打ち直させない)
    pub(super) fn open_rename(&mut self) {
        let Some(row) = self.tree.visible.get(self.tree.selected) else {
            return;
        };
        let from = row.path.clone();
        self.pending_g = false;
        // 畳まれた行 (`api/v1`) は row.name が連鎖全体なので、path の末尾から名前を取る。
        // UTF-8 でない名前は入力欄に正確に出せず、そのまま Enter すると置換文字入りの別名に
        // 化けてしまうので最初から断る
        let Some(name) = from.file_name().and_then(|n| n.to_str()) else {
            self.set_notice(
                t(
                    "UTF-8 でないファイル名はリネームできません",
                    "cannot rename a non-UTF-8 file name",
                ),
                true,
            );
            return;
        };
        let name = name.to_string();
        self.file_op = Some(FileOp::Rename { from });
        self.mode = Mode::Input {
            kind: InputKind::Rename,
            buffer: name,
        };
    }

    /// `D`。git の discard と違い fs から消すので必ず確認を挟む
    pub(super) fn confirm_delete(&mut self) {
        let Some(row) = self.tree.visible.get(self.tree.selected) else {
            return;
        };
        let path = row.path.clone();
        let is_dir = row.is_dir;
        let shown = self.relative_display(&path);
        let prompt = if is_dir {
            crate::tr!(
                "ディレクトリを削除しますか？ (配下も全て消えます)\n{shown}\n(復元できません)",
                "delete this directory? (everything inside is removed)\n{shown}\n(this cannot be undone)"
            )
        } else {
            crate::tr!(
                "ファイルを削除しますか？\n{shown}\n(復元できません)",
                "delete this file?\n{shown}\n(this cannot be undone)"
            )
        };
        self.pending_g = false;
        self.mode = Mode::Confirm {
            prompt,
            action: ConfirmAction::Delete { path, is_dir },
        };
    }

    pub(super) fn execute_delete(&mut self, path: PathBuf, is_dir: bool) {
        let result = if is_dir {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        let shown = self.relative_display(&path);
        match result {
            Ok(()) => {
                self.forget_open_under(&path);
                self.set_notice(
                    crate::tr!("削除しました: {shown}", "deleted: {shown}"),
                    false,
                );
                self.after_fs_write(None);
            }
            Err(e) => self.set_notice(
                crate::tr!("削除に失敗: {shown}: {e}", "delete failed: {shown}: {e}"),
                true,
            ),
        }
    }

    /// `y`: 選択行の root 相対パスをクリップボードへ (AI に「このファイルを直して」と渡す用)
    pub(super) fn copy_selected_path(&mut self) {
        let Some(row) = self.tree.visible.get(self.tree.selected) else {
            return;
        };
        let text = self.relative_display(&row.path);
        self.pending_g = false;
        match clipboard::copy(&text) {
            Ok(via) => self.set_notice(format!("copied path: {text} ({via})"), false),
            Err(e) => self.set_notice(format!("copy failed: {e}"), true),
        }
    }

    /// Input の Enter。作成/リネームを実行して結果を notice に出す。失敗時も Mode は
    /// 呼び出し側で Normal に戻る (入力を保ったまま留まらせるほどの長文ではない)
    pub(super) fn confirm_file_input(&mut self, kind: InputKind) {
        let Mode::Input { buffer, .. } = &self.mode else {
            return;
        };
        let input = buffer.trim().to_string();
        let Some(op) = self.file_op.take() else {
            return;
        };
        if input.is_empty() {
            return;
        }
        // リネームは「親を据え置いて名前だけ変える」操作なので 1 要素に限る (`a/b` を許すと
        // 黙って移動になり、ヘルプの案内と食い違う)
        let single = matches!(op, FileOp::Rename { .. });
        let rel = match validate_name(&input, single) {
            Ok(rel) => rel,
            Err(message) => {
                self.set_notice(message, true);
                return;
            }
        };
        match op {
            FileOp::Create { dir } => {
                let target = dir.join(&rel);
                self.create_entry(target, kind == InputKind::NewDir);
            }
            FileOp::Rename { from } => {
                let parent = from
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| self.root.clone());
                self.rename_entry(from, parent.join(&rel));
            }
        }
    }

    pub(super) fn cancel_file_input(&mut self) {
        self.file_op = None;
    }

    /// ステータスバーの入力欄に出す接頭辞。新規作成は「どこに作るか」が見えないと
    /// 選択行がファイルだった時にどの階層へ入るのか分からないので、親ディレクトリを添える
    pub fn file_op_label(&self, kind: InputKind) -> String {
        let dir = match &self.file_op {
            Some(FileOp::Create { dir }) => {
                let shown = self.relative_display(dir);
                if shown.is_empty() {
                    String::from("./")
                } else {
                    format!("{shown}/")
                }
            }
            _ => String::new(),
        };
        match kind {
            InputKind::NewFile => crate::tr!("新規ファイル {dir}", "new file {dir}"),
            InputKind::NewDir => crate::tr!("新規ディレクトリ {dir}", "new dir {dir}"),
            InputKind::Rename => t("リネーム: ", "rename: ").to_string(),
            _ => String::new(),
        }
    }

    fn create_entry(&mut self, target: PathBuf, is_dir: bool) {
        let shown = self.relative_display(&target);
        if let Err(message) = contained(&self.root, &target) {
            self.set_notice(message, true);
            return;
        }
        if target.exists() {
            self.set_notice(
                crate::tr!("既に存在します: {shown}", "already exists: {shown}"),
                true,
            );
            return;
        }
        let result = if is_dir {
            std::fs::create_dir_all(&target)
        } else {
            // `a/b/c.rs` のように途中のディレクトリごと作れるようにする (mkdir -p 相当)。
            // create_new で「存在チェックと作成の間に外から作られた」場合も上書きしない
            target
                .parent()
                .map(std::fs::create_dir_all)
                .unwrap_or(Ok(()))
                .and_then(|_| {
                    std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&target)
                        .map(|_| ())
                })
        };
        match result {
            Ok(()) => {
                self.set_notice(
                    crate::tr!("作成しました: {shown}", "created: {shown}"),
                    false,
                );
                self.after_fs_write(Some(&target));
                // 作ったファイルはそのまま読み書きしたいので右ペインにも開く。GIT レーンでは
                // open_selected が diff 側にしか届かないので、VIEW に戻った時のために viewer にも
                // 直接開いておく (VIEW では open_selected が同じ path を再度開いても no-op)
                if !is_dir {
                    self.viewer.open(&target, &self.root);
                    self.open_selected(&target);
                }
            }
            Err(e) => self.set_notice(
                crate::tr!("作成に失敗: {shown}: {e}", "create failed: {shown}: {e}"),
                true,
            ),
        }
    }

    fn rename_entry(&mut self, from: PathBuf, to: PathBuf) {
        if from == to {
            return;
        }
        let shown_from = self.relative_display(&from);
        let shown_to = self.relative_display(&to);
        if let Err(message) = contained(&self.root, &to) {
            self.set_notice(message, true);
            return;
        }
        if to.exists() {
            self.set_notice(
                crate::tr!("既に存在します: {shown_to}", "already exists: {shown_to}"),
                true,
            );
            return;
        }
        match rename_no_replace(&from, &to) {
            Ok(()) => {
                self.retarget_open(&from, &to);
                self.set_notice(
                    crate::tr!(
                        "リネームしました: {shown_from} → {shown_to}",
                        "renamed: {shown_from} → {shown_to}"
                    ),
                    false,
                );
                self.after_fs_write(Some(&to));
            }
            Err(e) => self.set_notice(
                crate::tr!(
                    "リネームに失敗: {shown_from}: {e}",
                    "rename failed: {shown_from}: {e}"
                ),
                true,
            ),
        }
    }

    // 書き込み後の共通の後始末。ツリー・git status は他の書き込み系操作と同じ rescan_now に
    // 相乗りさせ、横断検索の一覧は構造が変わったので捨てる (FS 監視が無い環境でも効かせる)。
    // reveal は再走査の後でないと新しいパスがツリーに無い
    fn after_fs_write(&mut self, reveal: Option<&Path>) {
        self.grep.invalidate();
        self.rescan_now();
        if let Some(path) = reveal {
            self.tree.reveal(path);
        }
    }

    // 消えたパス (配下含む) を右ペインで開いていたら閉じる。削除後の内容を出し続けない
    fn forget_open_under(&mut self, path: &Path) {
        let open = self
            .viewer
            .current
            .as_ref()
            .is_some_and(|open| open.path.starts_with(path));
        if open {
            self.viewer.close();
        }
    }

    // リネーム元 (配下含む) を開いていたら新しいパスで開き直す。履歴・cache の古いパスは
    // 読み直せないだけで害は無いので触らない
    fn retarget_open(&mut self, from: &Path, to: &Path) {
        let Some(open) = self.viewer.current.as_ref() else {
            return;
        };
        let Ok(rest) = open.path.strip_prefix(from) else {
            return;
        };
        let moved = to.join(rest);
        self.viewer.close();
        // GIT レーンでは open_selected が diff 側にしか届かない (create_entry と同じ理由で
        // viewer にも直接開く)
        self.viewer.open(&moved, &self.root);
        self.open_selected(&moved);
    }

    fn relative_display(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .display()
            .to_string()
    }
}

/// 既に存在する宛先を上書きしない rename。exists の事前確認と rename の間に外から同名が
/// 作られると `fs::rename` はそれを黙って置き換えるため、ファイルは hard_link (宛先があれば
/// 必ず失敗する) + 元の削除で原子的に移す。hard_link を持たないファイルシステムでは
/// (AlreadyExists 以外の失敗) 通常の rename に落とす。ディレクトリは hard_link できないので
/// rename のまま — 空でないディレクトリへの rename は OS が拒否するため、置き換わりうるのは
/// 空ディレクトリだけ
fn rename_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        return std::fs::rename(from, to);
    }
    match std::fs::hard_link(from, to) {
        Ok(()) => std::fs::remove_file(from),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(e),
        Err(_) => std::fs::rename(from, to),
    }
}

/// 書き込み先が実体として root の中にあるかを、symlink を解決した上で確かめる。
/// `link/new` のように途中の symlink がツリーの外を指していると、字面の join では root 配下に
/// 見えたまま外へ書いてしまう。まだ無い末尾は解決できないので、存在する最も深い祖先を
/// canonicalize して root と突き合わせる (途中で作るディレクトリは自分が作る実体なので
/// symlink になりえない)。確認と書き込みの間に symlink が差し替えられる競合までは防げない
fn contained(root: &Path, target: &Path) -> Result<(), String> {
    let outside = || {
        t(
            "symlink 越しにツリーの外へは書き込めません",
            "refusing to write outside the tree through a symlink",
        )
        .to_string()
    };
    let canonical_root = std::fs::canonicalize(root).map_err(|_| outside())?;
    let existing = target
        .ancestors()
        .find(|p| p.exists())
        .ok_or_else(outside)?;
    let resolved = std::fs::canonicalize(existing).map_err(|_| outside())?;
    if resolved.starts_with(&canonical_root) {
        Ok(())
    } else {
        Err(outside())
    }
}

// 入力を root 配下に閉じる。`..` や絶対パスを通すとツリーの外を書き換えてしまう。
// single は「1 要素だけ許す」(リネーム)
fn validate_name(input: &str, single: bool) -> Result<PathBuf, String> {
    // 末尾の `/` は「ディレクトリのつもり」の癖として黙って落とす。先頭の `/` は絶対パスなので
    // 落とさず (root 配下に読み替えると意図と違う場所に作る) 下の判定で弾く
    let rel = Path::new(input.trim().trim_end_matches('/'));
    if rel.as_os_str().is_empty() {
        return Err(t("名前が空です", "empty name").to_string());
    }
    for component in rel.components() {
        match component {
            Component::Normal(_) => {}
            _ => {
                return Err(t(
                    "相対パスの名前だけ使えます (.. や絶対パスは不可)",
                    "only a relative name is allowed (no .. or absolute paths)",
                )
                .to_string());
            }
        }
    }
    if single && rel.components().count() > 1 {
        return Err(t(
            "リネームは名前だけです (別のディレクトリへは動かせません)",
            "rename takes a bare name (it cannot move to another directory)",
        )
        .to_string());
    }
    Ok(rel.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::{contained, rename_no_replace, validate_name};

    #[test]
    fn validate_rejects_escape() {
        assert!(validate_name("../x", false).is_err());
        assert!(validate_name("/etc/passwd", false).is_err());
        assert!(validate_name("", false).is_err());
        assert!(validate_name("   ", false).is_err());
    }

    #[test]
    fn validate_rename_is_single_component() {
        assert!(validate_name("a/b", true).is_err());
        assert!(validate_name("b", true).is_ok());
    }

    #[test]
    fn contained_follows_symlinks() {
        let tmp = std::env::temp_dir().join(format!("fv-contained-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let root = tmp.join("root");
        let outside = tmp.join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
            assert!(contained(&root, &root.join("link/new")).is_err());
        }
        assert!(contained(&root, &root.join("a/b/new")).is_ok());
        assert!(contained(&root, &root.join("new")).is_ok());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn rename_no_replace_refuses_existing() {
        let tmp = std::env::temp_dir().join(format!("fv-rename-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("a"), "a").unwrap();
        std::fs::write(tmp.join("b"), "b").unwrap();
        assert!(rename_no_replace(&tmp.join("a"), &tmp.join("b")).is_err());
        assert_eq!(std::fs::read_to_string(tmp.join("b")).unwrap(), "b");
        assert!(rename_no_replace(&tmp.join("a"), &tmp.join("c")).is_ok());
        assert!(!tmp.join("a").exists());
        assert_eq!(std::fs::read_to_string(tmp.join("c")).unwrap(), "a");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn validate_accepts_nested() {
        assert_eq!(
            validate_name("a/b/c.rs", false).unwrap(),
            std::path::PathBuf::from("a/b/c.rs")
        );
        assert_eq!(
            validate_name("trailing/", true).unwrap(),
            std::path::PathBuf::from("trailing")
        );
    }
}
