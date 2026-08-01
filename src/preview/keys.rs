//! シーン定義で状態を作るためのキー列 DSL。プレビューは App の内部を直接いじらず
//! 「実際に押されるキー」で状態を組み立てる — そうしないと keys.rs の優先順位を通らない
//! 経路だけがプレビューで綺麗に見える、という一番まずいズレが起きる。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// `"<S-Tab>jj<CR>"` のような文字列を KeyEvent 列にする。
/// 素の文字はそのまま 1 キー、`<...>` は特殊キー・修飾付きキー
pub fn parse(script: &str) -> Vec<KeyEvent> {
    let mut events = Vec::new();
    let mut chars = script.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '<' {
            events.push(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
            continue;
        }
        let mut token = String::new();
        for c in chars.by_ref() {
            if c == '>' {
                break;
            }
            token.push(c);
        }
        if let Some(event) = token_event(&token) {
            events.push(event);
        }
    }
    events
}

fn token_event(token: &str) -> Option<KeyEvent> {
    // <C-p> / <A-2>: 修飾 + 1 文字
    if let Some(rest) = token.strip_prefix("C-") {
        let c = rest.chars().next()?;
        return Some(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
    }
    if let Some(rest) = token.strip_prefix("A-") {
        let c = rest.chars().next()?;
        return Some(KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT));
    }
    let (code, modifiers) = match token {
        "S-Tab" => (KeyCode::BackTab, KeyModifiers::SHIFT),
        "Tab" => (KeyCode::Tab, KeyModifiers::NONE),
        "CR" | "Enter" => (KeyCode::Enter, KeyModifiers::NONE),
        "Esc" => (KeyCode::Esc, KeyModifiers::NONE),
        "Space" => (KeyCode::Char(' '), KeyModifiers::NONE),
        "BS" => (KeyCode::Backspace, KeyModifiers::NONE),
        "Up" => (KeyCode::Up, KeyModifiers::NONE),
        "Down" => (KeyCode::Down, KeyModifiers::NONE),
        "Left" => (KeyCode::Left, KeyModifiers::NONE),
        "Right" => (KeyCode::Right, KeyModifiers::NONE),
        "lt" => (KeyCode::Char('<'), KeyModifiers::NONE),
        _ => return None,
    };
    Some(KeyEvent::new(code, modifiers))
}
