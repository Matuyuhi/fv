//! `fv --preview`: TUI を起動せずに 1 フレームを描き出す静的プレビュー。
//! Compose の `@Preview` / SwiftUI の Preview と同じ狙いで、「実装 → 保存 → 見た目を確認」を
//! アプリの操作なしで回せるようにするための開発用の入口。
//!
//! 実装は ratatui の `TestBackend` (メモリ上の Buffer) に `ui::draw` をそのまま通すだけで、
//! **描画コードにプレビュー専用の分岐を一切足さない** — プレビューにだけ都合の良い経路を
//! 作ると「プレビューでは直っているのに実物が直っていない」が起きるため。

mod fixture;
mod keys;
mod render;
mod scene;
mod snapshot;

use std::error::Error;
use std::io::{self, IsTerminal, Write};
use std::path::Path;

use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::app::App;
use crate::config::Config;

const DEFAULT_SIZE: (u16, u16) = (110, 32);

#[derive(Default)]
pub struct Options {
    /// `--preview` が指定されたか。false ならプレビューではなく通常起動
    pub enabled: bool,
    /// 空ならシーン一覧を出すだけ。"all" で全シーン
    pub scenes: Vec<String>,
    /// 指定があればシーン側の希望サイズより優先する
    pub size: Option<(u16, u16)>,
    /// None なら出力先が端末かどうかで決める (パイプ・リダイレクトでは色を落とす)
    pub color: Option<bool>,
    /// stdout ではなく tests/snapshots/ へ書き出す (CI の UI 差分検出用)。
    /// シーン無指定は一覧ではなく全シーンの意味になる
    pub update_snapshots: bool,
}

impl Options {
    /// プレビュー専用オプションの解釈。main.rs 側にプレビューの知識を持ち込まないため
    /// (feature を切った製品ビルドでは main.rs から丸ごと消え、未知のオプションとして
    /// 弾かれる) ここに置く。消費したら true
    pub fn take_flag(
        &mut self,
        arg: &str,
        rest: &mut impl Iterator<Item = String>,
    ) -> Result<bool, Box<dyn Error>> {
        match arg {
            "--preview" => self.enabled = true,
            "--update-snapshots" => self.update_snapshots = true,
            "--color" => self.color = Some(true),
            "--no-color" => self.color = Some(false),
            "--size" => {
                let value = rest
                    .next()
                    .ok_or("--size requires WxH (e.g. --size 120x40)")?;
                self.size = Some(parse_size(&value)?);
            }
            _ if arg.starts_with("--size=") => {
                self.size = Some(parse_size(arg.trim_start_matches("--size="))?);
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}

// --size 120x40 / --size=120x40 の両方を受ける
fn parse_size(value: &str) -> Result<(u16, u16), Box<dyn Error>> {
    let (w, h) = value
        .split_once(['x', 'X'])
        .ok_or_else(|| format!("invalid --size: {value} (expected WxH, e.g. 120x40)"))?;
    Ok((w.trim().parse()?, h.trim().parse()?))
}

pub fn run(options: Options) -> Result<(), Box<dyn Error>> {
    let mut out = io::stdout();
    // シーン無指定: 通常は一覧を出すだけ。スナップショット更新は「全部」の意味にする
    // (cargo preview --update-snapshots だけで CI と同じものが出せるように)
    if options.scenes.is_empty() && !options.update_snapshots {
        print_catalog(&mut out)?;
        return Ok(());
    }
    let selected = resolve_scenes(&options.scenes)?;
    // スナップショットは色を持たない (ANSI を含めると git diff が読めなくなるため)
    let color = !options.update_snapshots && options.color.unwrap_or_else(|| out.is_terminal());
    isolate_env();
    let root = fixture::build()?;

    for scene in &selected {
        let size = options.size.or(scene.size).unwrap_or(DEFAULT_SIZE);
        let mut body = draw_scene(scene, &root, size, color);
        if options.update_snapshots {
            body = snapshot::normalize(&body);
        }
        let card = render::card(scene.name, scene.description, size.0, size.1, &body, color);
        if options.update_snapshots {
            snapshot::write(scene.name, &card)?;
        } else {
            write!(out, "{card}")?;
        }
    }
    if options.update_snapshots {
        report_snapshots(&mut out, &selected)?;
    }
    out.flush()?;
    Ok(())
}

// 更新モードの結果表示。全シーンを書いた時だけ、消えたシーンの残骸も掃除する
// (部分更新でそれをやると、指定しなかったシーンのスナップショットまで消えてしまう)
fn report_snapshots(
    out: &mut impl Write,
    selected: &[&scene::Scene],
) -> Result<(), Box<dyn Error>> {
    let names: Vec<&str> = selected.iter().map(|s| s.name).collect();
    if selected.len() == scene::SCENES.len() {
        for path in snapshot::prune(&names)? {
            writeln!(out, "removed {}", path.display())?;
        }
    }
    writeln!(
        out,
        "wrote {} snapshots to {}",
        names.len(),
        snapshot::dir().display()
    )?;
    Ok(())
}

fn resolve_scenes(names: &[String]) -> Result<Vec<&'static scene::Scene>, Box<dyn Error>> {
    if names.is_empty() || names.iter().any(|name| name == "all") {
        return Ok(scene::SCENES.iter().collect());
    }
    names
        .iter()
        .map(|name| {
            scene::find(name)
                .ok_or_else(|| format!("unknown preview scene: {name} (一覧: fv --preview)").into())
        })
        .collect()
}

// プレビューの出力を実行環境から切り離す。App::new より前 = スレッドを 1 つも
// 起こしていない時点で呼ぶ
fn isolate_env() {
    let scratch = std::env::temp_dir().join("fv-preview-config");
    unsafe {
        // 設定の読み書きを使い捨てのディレクトリへ逃がす。プレビューのキー列には w (折返し) の
        // ように persist_config を呼ぶものがあり、そのままだと利用者の ~/.config/fv/config を
        // 書き換えてしまう
        std::env::set_var("XDG_CONFIG_HOME", &scratch);
        // git の相対日時 ("3 days ago") は gettext の翻訳対象。日本語ロケールの手元と
        // C ロケールの CI で表示が変わると、UI が同じでもスナップショットが食い違う
        std::env::set_var("LC_ALL", "C");
    }
}

fn print_catalog(out: &mut impl Write) -> io::Result<()> {
    writeln!(
        out,
        "usage: fv --preview <scene>... [--size WxH] [--no-color]"
    )?;
    writeln!(out, "       fv --preview all\n")?;
    writeln!(out, "scenes:")?;
    let width = scene::SCENES
        .iter()
        .map(|s| s.name.len())
        .max()
        .unwrap_or(0);
    for scene in scene::SCENES {
        writeln!(out, "  {:width$}  {}", scene.name, scene.description)?;
    }
    Ok(())
}

/// 1 シーンを描いて Buffer を文字列行に落とす。
/// 「描画 → setup → 描画」と 2 回描くのは、viewport の高さ・幅やペインの Rect を ui が
/// App へ書き戻す構造 (CLAUDE.md「描画は自前スライス」) に合わせるため。1 回目で実測値が
/// 入り、setup のキー列 (Ctrl+d のような height 依存の操作) が実際のアプリと同じ値を見る
fn draw_scene(scene: &scene::Scene, root: &Path, size: (u16, u16), color: bool) -> Vec<String> {
    let config = Config {
        // プレビューを実行した端末の環境 (Nerd Font の有無) に出力が左右されないよう固定する
        icons: false,
        ..Config::default()
    };
    let mut app = App::new(root.to_path_buf(), config, false);
    let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).expect("test backend");
    let mut draw = |app: &mut App| {
        let _ = terminal.draw(|frame| crate::ui::draw(frame, app));
    };
    draw(&mut app);
    (scene.setup)(&mut app);
    settle(&mut app);
    draw(&mut app);
    render::buffer_lines(terminal.backend().buffer(), color)
}

// Finder の候補は別スレッドの全走査で埋まるため、開いた直後は "scanning..." のままになる。
// 走査中のシーンだけ完了を待って、プレビューが毎回違う中途状態を写さないようにする
// (走査を起こしていないシーンでは scanning() が false のまま = 1 度も待たない)
fn settle(app: &mut App) {
    for _ in 0..200 {
        if !app.file_index.scanning() {
            break;
        }
        app.on_tick();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    app.on_tick();
}
