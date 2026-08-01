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

use std::error::Error;
use std::io::{self, IsTerminal, Write};
use std::path::Path;

use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::app::App;
use crate::config::Config;

const DEFAULT_SIZE: (u16, u16) = (110, 32);

pub struct Options {
    /// 空ならシーン一覧を出すだけ。"all" で全シーン
    pub scenes: Vec<String>,
    /// 指定があればシーン側の希望サイズより優先する
    pub size: Option<(u16, u16)>,
    /// None なら出力先が端末かどうかで決める (パイプ・リダイレクトでは色を落とす)
    pub color: Option<bool>,
}

pub fn run(options: Options) -> Result<(), Box<dyn Error>> {
    let mut out = io::stdout();
    if options.scenes.is_empty() {
        print_catalog(&mut out)?;
        return Ok(());
    }
    let selected = resolve_scenes(&options.scenes)?;
    let color = options.color.unwrap_or_else(|| out.is_terminal());
    isolate_config();
    let root = fixture::build()?;

    for scene in selected {
        let size = options.size.or(scene.size).unwrap_or(DEFAULT_SIZE);
        let body = draw_scene(scene, &root, size, color);
        write!(
            out,
            "{}",
            render::card(scene.name, scene.description, size.0, size.1, &body, color)
        )?;
    }
    out.flush()?;
    Ok(())
}

fn resolve_scenes(names: &[String]) -> Result<Vec<&'static scene::Scene>, Box<dyn Error>> {
    if names.iter().any(|name| name == "all") {
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

// 設定の読み書きを使い捨てのディレクトリへ逃がす。プレビュー中のキー列には w (折返し) の
// ように persist_config を呼ぶものがあり、そのままだと利用者の ~/.config/fv/config を
// 書き換えてしまう。App::new より前 = スレッドを 1 つも起こしていない時点で呼ぶ
fn isolate_config() {
    let scratch = std::env::temp_dir().join("fv-preview-config");
    unsafe {
        std::env::set_var("XDG_CONFIG_HOME", &scratch);
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
