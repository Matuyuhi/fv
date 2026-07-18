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
    Run { root: PathBuf, show_hidden: bool },
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
                "fv - read-only TUI code viewer\n\nusage: fv [options] [dir]\n\noptions:\n  -a, --hidden  show hidden files and directories\n  -h, --help    print help\n  -V, --version print version\n\npress ? inside the app for keybindings"
            );
        }
        Command::Run { root, show_hidden } => run_app(root, show_hidden)?,
    }
    Ok(())
}

fn run_app(root: PathBuf, show_hidden: bool) -> Result<(), Box<dyn Error>> {
    let mut app = App::new(root, show_hidden);
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

    for arg in args {
        match arg.as_str() {
            "--version" | "-V" => return Ok(Command::Version),
            "--help" | "-h" => return Ok(Command::Help),
            "--hidden" | "-a" => show_hidden = true,
            _ if arg.starts_with('-') => return Err(format!("unknown option: {arg}").into()),
            _ => {
                if root.replace(PathBuf::from(arg)).is_some() {
                    return Err("only one directory can be specified".into());
                }
            }
        }
    }

    let root = resolve_root(root.unwrap_or_else(|| PathBuf::from(".")))?;
    Ok(Command::Run { root, show_hidden })
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
