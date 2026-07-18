mod app;
mod finder;
mod git;
mod tree;
mod ui;
mod viewer;
mod watch;

use std::env;
use std::error::Error;
use std::io;
use std::panic;
use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use app::App;

enum Command {
    Run {
        root: PathBuf,
        show_hidden: bool,
        icons: bool,
    },
    Help,
    Version,
}

fn main() -> Result<(), Box<dyn Error>> {
    match parse_command(env::args().skip(1))? {
        Command::Version => {
            println!("fv {}", env!("CARGO_PKG_VERSION"));
        }
        Command::Help => {
            println!(
                "fv - read-only TUI code viewer\n\nusage: fv [options] [dir]\n\noptions:\n  -a, --hidden  show hidden files and directories\n      --icons     show Nerd Font file icons (default: auto by terminal / FV_ICONS)\n      --no-icons  disable file icons\n  -h, --help    print help\n  -V, --version print version\n\npress ? inside the app for keybindings"
            );
        }
        Command::Run {
            root,
            show_hidden,
            icons,
        } => run_app(root, show_hidden, icons)?,
    }
    Ok(())
}

fn run_app(root: PathBuf, show_hidden: bool, icons: bool) -> Result<(), Box<dyn Error>> {
    let mut app = App::new(root, show_hidden, icons);
    install_panic_hook();
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;

    let result = run(&mut terminal, &mut app);
    restore_terminal();
    result
}

fn run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<(), Box<dyn Error>> {
    loop {
        terminal.draw(|frame| ui::draw(frame, app))?;
        // poll がタイムアウトしても 100ms 周期でループが回り、その都度 watcher を drain する。
        // これがそのまま再描画・自動リロードのポーリング間隔にもなる
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => app.on_key(key),
                Event::Mouse(mouse) => app.on_mouse(mouse),
                _ => {}
            }
        }
        app.on_tick();
        if app.should_quit {
            return Ok(());
        }
    }
}

fn parse_command(args: impl Iterator<Item = String>) -> Result<Command, Box<dyn Error>> {
    let mut root = None;
    let mut show_hidden = false;
    let mut icons = None;

    for arg in args {
        match arg.as_str() {
            "--version" | "-V" => return Ok(Command::Version),
            "--help" | "-h" => return Ok(Command::Help),
            "--hidden" | "-a" => show_hidden = true,
            "--icons" => icons = Some(true),
            "--no-icons" => icons = Some(false),
            _ if arg.starts_with('-') => return Err(format!("unknown option: {arg}").into()),
            _ => {
                if root.replace(PathBuf::from(arg)).is_some() {
                    return Err("only one directory can be specified".into());
                }
            }
        }
    }

    let root = resolve_root(root.unwrap_or_else(|| PathBuf::from(".")))?;
    let icons = icons.unwrap_or_else(icons_default);
    Ok(Command::Run {
        root,
        show_hidden,
        icons,
    })
}

// フラグ未指定時のアイコン有効判定。FV_ICONS があればそれに従い、
// なければ「Nerd Font シンボルを同梱していて未設定でも豆腐にならないターミナル」に限り有効化する。
// フォント自体の有無は端末に照会できない (未収録グリフも 1 セル幅で描画されるため
// カーソル位置プローブでも判別不能)。それ以外の端末は --icons / FV_ICONS=1 で opt-in する
fn icons_default() -> bool {
    if let Ok(v) = env::var("FV_ICONS") {
        return !matches!(v.as_str(), "" | "0" | "false" | "off");
    }
    let term_program = env::var("TERM_PROGRAM").unwrap_or_default();
    if matches!(term_program.as_str(), "WezTerm" | "ghostty") {
        return true;
    }
    // kitty は 0.32 以降 Nerd Font シンボルを同梱している
    env::var("TERM").is_ok_and(|t| t.contains("kitty") || t.contains("ghostty"))
        || env::var("KITTY_WINDOW_ID").is_ok()
}

fn resolve_root(root: PathBuf) -> Result<PathBuf, Box<dyn Error>> {
    let root = root.canonicalize()?;
    if !root.is_dir() {
        return Err(format!("{} is not a directory", root.display()).into());
    }
    Ok(root)
}

fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
}

// panic 時も端末を alternate screen / raw mode のまま残さないための hook。
// 復元してから既定の hook に渡すことで、panic メッセージが通常画面に出る。
fn install_panic_hook() {
    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        restore_terminal();
        default_hook(info);
    }));
}
