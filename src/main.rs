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
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use app::App;

fn main() -> Result<(), Box<dyn Error>> {
    // TUI に入る前に処理するフラグ。brew の formula test も --version に依存している
    match env::args().nth(1).as_deref() {
        Some("--version" | "-V") => {
            println!("fv {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Some("--help" | "-h") => {
            println!("fv - read-only TUI code viewer\n\nusage: fv [dir]\n\npress ? inside the app for keybindings");
            return Ok(());
        }
        _ => {}
    }
    let root = resolve_root()?;
    let mut app = App::new(root);

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

fn resolve_root() -> Result<PathBuf, Box<dyn Error>> {
    let arg = env::args().nth(1).unwrap_or_else(|| String::from("."));
    let root = PathBuf::from(&arg).canonicalize()?;
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
