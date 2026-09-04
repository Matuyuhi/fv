//! `fv --perf`: キー入力 → 再描画 1 サイクルの所要時間を、TUI を起動せずに測る開発用の入口。
//!
//! 測っているのは **1 打鍵ぶんの画面を組み立てるコスト**で、preview と同じく `TestBackend` へ
//! `shell::draw` をそのまま通す — 計測専用の描画経路を作ると「ベンチだけ速い」が起きるため。
//! 状態もキー列 (preview/keys.rs) で組み立てるので、`app/keys.rs` の優先順位を通らない
//! 経路を測ってしまうこともない。
//!
//! pty 経由で実際に端末を起動する測り方は、起動待ちの sleep と端末の応答が支配的になって
//! ノイズが大きすぎた。ここは 1 プロセスの中で完結するので、同じ入力に対して常に同じ量の
//! 仕事をする (時刻・端末・ネットワークに依存しない)。
//!
//! CI では base と head の両方でこれを走らせて差分を PR コメントに出す (`scripts/ci-perf.sh`)。
//! **絶対値は runner の同居負荷でぶれる**ので、意味があるのは「同じ runner で連続して
//! 測った 2 点の比」だけ。
//!
//! 対象は 2 つの合成物に分けてある。git を絡めない方 (build_fixture) が VIEW/EDIT の
//! 素の描画コストで、git repo の方 (build_git_fixture) は「git が絡んで初めて通る経路」
//! — EDIT レーンの未保存バッファのライブ diff (baseline が無いと計算自体が走らない) と、
//! GIT レーンの diff ペイン — を測るためのもの。分けているのは、git repo にすると
//! ファイルを開くところで `git diff` の実行時間が混ざり、素の描画コストが読めなくなるため。

use std::error::Error;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::KeyEvent;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::app::App;
use crate::config::Config;

use super::keys;

/// 端末サイズ。可視範囲だけを組み立てる設計 (component/viewer/render.rs) では
/// 1 フレームのコストがここに比例するので、測る側で固定する
const SIZE: (u16, u16) = (120, 40);
/// 繰り返し回数。最小値を採ることで、同居プロセスの負荷や allocator の揺れを落とす
const REPEATS: usize = 3;
/// 合成ファイルの行数。「開くコストが行数に比例しない」ことの確認も兼ねて大きめに取る
const FIXTURE_LINES: usize = 20_000;
const TYPE_OPS: usize = 200;
const SCROLL_OPS: usize = 300;
/// git repo 側の合成ファイルの行数と、そのうち書き換える連続した行数。
/// 「AI がまとまった範囲を書き直したものを手直しする」という実際の使い方に寄せてある —
/// 全体に散らした変更にすると LCS が上限で諦めて全行変更扱いになり、測っているものが
/// 「差分の表示コスト」から「巨大な HashSet の組み立て」にすり替わる
const GIT_FIXTURE_LINES: usize = 4_000;
const GIT_FIXTURE_EDITED: usize = 400;

// 固定パスを消す以上、目印の無いディレクトリは触らない (preview/fixture.rs と同じ作法)
const MARKER: &str = ".fv-perf-fixture";

struct Case {
    name: &'static str,
    /// この計測が見るディレクトリ。git が絡んで初めて通る経路だけ Git を選ぶ
    fixture: Fixture,
    /// 既定サイズでは測れないケースだけ指定する (side-by-side は 1 カラムが
    /// 40 桁を切ると inline に自動フォールバックしてしまい、別のものを測ってしまう)
    size: Option<(u16, u16)>,
    /// 計測前に流すキー列。レーン・スクロール位置を作るだけで、時間には数えない
    setup: &'static str,
    /// 計測対象。1 キーごとに再描画する (main.rs のイベントループと同じ粒度)
    measured: String,
}

#[derive(Clone, Copy, PartialEq)]
enum Fixture {
    /// git を絡めない素の描画コスト
    Plain,
    /// ライブ diff・diff ペインなど、git repo でないと走らない経路
    Git,
}

pub fn run(out: &mut impl Write) -> Result<(), Box<dyn Error>> {
    let plain = build_fixture()?;
    let git = build_git_fixture()?;
    writeln!(out, "# case\tops\ttotal_ms\tper_op_ms")?;
    for case in cases() {
        let root = match case.fixture {
            Fixture::Plain => &plain,
            Fixture::Git => &git,
        };
        let events = keys::parse(&case.measured);
        let ops = events.len().max(1);
        // 最小値: 遅い側の外れ値は必ず外乱なので、速い方が「この実装の実力」に近い
        let best = (0..REPEATS)
            .map(|_| measure(root, &case, &events))
            .min()
            .unwrap_or_default();
        let total = best.as_secs_f64() * 1000.0;
        writeln!(
            out,
            "{}\t{}\t{:.3}\t{:.4}",
            case.name,
            ops,
            total,
            total / ops as f64
        )?;
    }
    out.flush()?;
    Ok(())
}

// 合成リポジトリにはファイルが 1 つしか無いので、起動直後の選択がそのままそのファイルになる。
// <CR> で開く → <Tab> で右ペインへ、という実際の操作列だけで各レーンに入れる
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "open",
            fixture: Fixture::Plain,
            size: None,
            setup: "",
            measured: "<CR>".to_string(),
        },
        Case {
            name: "type",
            fixture: Fixture::Plain,
            size: None,
            setup: "<CR><Tab>e",
            measured: "x".repeat(TYPE_OPS),
        },
        Case {
            name: "scroll-down",
            fixture: Fixture::Plain,
            size: None,
            setup: "<CR><Tab>",
            measured: "j".repeat(SCROLL_OPS),
        },
        Case {
            name: "scroll-up",
            fixture: Fixture::Plain,
            size: None,
            setup: "<CR><Tab>G",
            measured: "k".repeat(SCROLL_OPS),
        },
        // baseline (HEAD 版) がある状態のタイピング。変更行マークのライブ diff が
        // 1 打鍵ごとに走るので、plain の type との差がそのままその計算のコストになる
        Case {
            name: "type-tracked",
            fixture: Fixture::Git,
            size: None,
            setup: "<CR><Tab>e",
            measured: "x".repeat(TYPE_OPS),
        },
        // GIT レーンの diff ペインを 1 行ずつ辿る
        Case {
            name: "git-scroll",
            fixture: Fixture::Git,
            size: None,
            setup: "<S-Tab><CR><Tab>",
            measured: "j".repeat(SCROLL_OPS),
        },
        // side-by-side + 折返しは左右の視覚行数を揃え直す唯一の表示で、
        // ここだけ「1 打鍵のコストが diff 全体の大きさに比例する」形になりやすい
        Case {
            name: "git-scroll-side-wrap",
            fixture: Fixture::Git,
            // 既定の 120 桁だとカラムが 40 桁ちょうどで、少し変わるだけで inline に落ちる
            size: Some((140, 40)),
            setup: "<S-Tab><CR><Tab>vw",
            measured: "j".repeat(SCROLL_OPS),
        },
    ]
}

// 1 回ぶんの計測。App はケースごと・繰り返しごとに作り直す (前の計測で温まった
// ハイライトのキャッシュを持ち越すと、2 回目以降だけ速いという嘘の数字になる)
fn measure(root: &Path, case: &Case, events: &[KeyEvent]) -> Duration {
    let config = Config {
        // 実行環境 (Nerd Font の有無・保存済み設定) で仕事量が変わらないよう固定する
        icons: false,
        lang: super::preview_lang(),
        ..Config::default()
    };
    let mut app = App::new(root.to_path_buf(), config, false);
    let (cols, rows) = case.size.unwrap_or(SIZE);
    let mut terminal = Terminal::new(TestBackend::new(cols, rows)).expect("test backend");
    let mut draw = |app: &mut App| {
        let _ = terminal.draw(|frame| crate::shell::draw(frame, app));
    };
    // 1 回目の描画で viewport の実測値が App に入る (CLAUDE.md「描画は自前スライス」)。
    // これを挟まないと setup の高さ依存のキーが 0 行の viewport を見てしまう
    draw(&mut app);
    for key in keys::parse(case.setup) {
        app.on_key(key);
        draw(&mut app);
    }
    let started = Instant::now();
    for key in events {
        app.on_key(*key);
        draw(&mut app);
    }
    started.elapsed()
}

// 実プロジェクトを測ると「その時の作業状態」で数字が動いて 2 点間の比較にならないため、
// preview と同じく固定の合成物だけを対象にする。git repo にはしない — git status の
// 実行時間が混ざるうえ、測りたいのは描画のコストなので
fn build_fixture() -> Result<PathBuf, Box<dyn Error>> {
    let root = std::env::temp_dir().join("fv-perf-fixture");
    if root.join(MARKER).exists() {
        fs::remove_dir_all(&root)?;
    }
    fs::create_dir_all(&root)?;
    fs::write(root.join(MARKER), "generated by `fv --perf`\n")?;

    let mut text = String::with_capacity(FIXTURE_LINES * 48);
    for i in 0..FIXTURE_LINES / 2 {
        text.push_str(&format!("/// generated item {i}\n"));
        text.push_str(&format!(
            "pub fn item_{i}(x: &str) -> String {{ format!(\"value {{x}} {i}\") }}\n"
        ));
    }
    fs::write(root.join("big.rs"), text)?;
    Ok(root)
}

// git が絡んで初めて通る経路 (EDIT のライブ diff・GIT レーンの diff ペイン) 用。
// 1 ファイルをコミットしてから作業ツリー側だけ書き換え、「HEAD との差分がある
// tracked ファイル」を 1 つだけ作る — 測りたいのは差分の**表示**のコストなので、
// 状態の種類は増やさず diff が十分な行数になることだけを狙う
fn build_git_fixture() -> Result<PathBuf, Box<dyn Error>> {
    let root = std::env::temp_dir().join("fv-perf-git-fixture");
    if root.join(MARKER).exists() {
        fs::remove_dir_all(&root)?;
    }
    fs::create_dir_all(&root)?;
    fs::write(root.join(MARKER), "generated by `fv --perf`\n")?;

    let file = root.join("tracked.rs");
    fs::write(&file, git_fixture_text(false))?;
    git(&root, &["init", "-q", "-b", "main"])?;
    git(&root, &["add", "-A"])?;
    git(&root, &["commit", "-q", "-m", "initial"])?;
    fs::write(&file, git_fixture_text(true))?;

    // preview/fixture.rs と同じく実体パスへ寄せる ($TMPDIR がシンボリックリンクの
    // 環境で、git が返す toplevel とツリー走査のパスが食い違うのを避ける)
    Ok(root.canonicalize()?)
}

// 真ん中の連続した GIT_FIXTURE_EDITED 行だけを書き換えた版を返す
fn git_fixture_text(modified: bool) -> String {
    let edited = (GIT_FIXTURE_LINES / 2 - GIT_FIXTURE_EDITED / 2)..(GIT_FIXTURE_LINES / 2);
    let mut text = String::with_capacity(GIT_FIXTURE_LINES * 48);
    for i in 0..GIT_FIXTURE_LINES / 2 {
        text.push_str(&format!("/// generated item {i}\n"));
        let suffix = if modified && edited.contains(&i) {
            " // rewritten by the assistant"
        } else {
            ""
        };
        text.push_str(&format!(
            "pub fn item_{i}(x: &str) -> String {{ format!(\"value {{x}} {i}\") }}{suffix}\n"
        ));
    }
    text
}

// ユーザーの ~/.gitconfig (署名・hook・テンプレート) に左右されないよう、
// 必要な設定は全て -c で明示する (preview/fixture.rs と同じ作法)
fn git(root: &Path, args: &[&str]) -> Result<(), Box<dyn Error>> {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "-c",
            "user.name=fv perf",
            "-c",
            "user.email=perf@example.com",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(args)
        .status()?;
    if !status.success() {
        return Err(format!("git {} に失敗しました", args.join(" ")).into());
    }
    Ok(())
}
