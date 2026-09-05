use std::sync::atomic::{AtomicU8, Ordering};

mod en;
mod ja;
mod msg;

pub use msg::Msg;

// UI 文言の言語。プロセス全体で 1 つの値を static に持つのは、描画関数 (自分の状態しか
// 受け取らない設計) と背景スレッド (gh/git の失敗メッセージ) の両方から同じ値を読むためで、
// 引数で配って回ると全ての draw_* と notice の組み立てに引数の変更が及ぶ。
// 文言そのものは呼び出し側に置かず、キー (`Msg`) で引く。翻訳表は言語ごとに 1 ファイル
// (ja.rs / en.rs) で、`Msg` に対する match を網羅させることで「片方の言語だけ書き忘れた
// 文言」をコンパイルエラーにしている (以前の「対で書く」設計と同じ保証をキー方式で保つ)
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Lang {
    #[default]
    Ja,
    En,
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::Ja, Lang::En];

    /// config の値。表示 (設定画面) にもそのまま使う
    pub fn as_str(self) -> &'static str {
        match self {
            Lang::Ja => "ja",
            Lang::En => "en",
        }
    }

    pub fn parse(s: &str) -> Option<Lang> {
        match s.trim() {
            "ja" => Some(Lang::Ja),
            "en" => Some(Lang::En),
            _ => None,
        }
    }

    /// config に無い時の既定。gettext と同じ優先順 (LC_ALL > LC_MESSAGES > LANG) で
    /// ロケールを見て、日本語ならそのまま、それ以外は英語に倒す
    pub fn detect() -> Lang {
        for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
            if let Ok(v) = std::env::var(key)
                && !v.is_empty()
            {
                return if v.starts_with("ja") {
                    Lang::Ja
                } else {
                    Lang::En
                };
            }
        }
        Lang::En
    }

    pub fn next(self, delta: isize) -> Lang {
        let idx = Lang::ALL.iter().position(|l| *l == self).unwrap_or(0) as isize;
        let len = Lang::ALL.len() as isize;
        Lang::ALL[(idx + delta).rem_euclid(len) as usize]
    }
}

pub fn set(lang: Lang) {
    CURRENT.store(lang as u8, Ordering::Relaxed);
}

pub fn current() -> Lang {
    match CURRENT.load(Ordering::Relaxed) {
        1 => Lang::En,
        _ => Lang::Ja,
    }
}

/// 固定文言。今の言語の表からキーで引く
pub fn t(msg: Msg) -> &'static str {
    match current() {
        Lang::Ja => ja::text(msg),
        Lang::En => en::text(msg),
    }
}

/// 埋め込みのある文言。文言側の `{name}` を同名の引数で置き換える。
/// 表の文字列は `&'static str` なので `format!` には渡せず、名前で引く自前の置換にしてある
/// (`tr!` マクロから呼ぶ。位置引数は持たず、両言語で同じ名前を使うことを placeholders_match
/// のテストで担保する)
pub fn fmt(msg: Msg, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    let text = t(msg);
    let mut out = String::with_capacity(text.len() + 16);
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) => {
                let name = &after[..close];
                match args.iter().find(|(k, _)| *k == name) {
                    Some((_, value)) => {
                        use std::fmt::Write;
                        let _ = write!(out, "{value}");
                    }
                    None => {
                        debug_assert!(false, "placeholder {{{name}}} に対応する引数が無い");
                        out.push('{');
                        out.push_str(name);
                        out.push('}');
                    }
                }
                rest = &after[close + 1..];
            }
            None => {
                out.push_str(&rest[open..]);
                return out;
            }
        }
    }
    out.push_str(rest);
    out
}

/// 文言中の `{name}` を集める (ja/en のプレースホルダが揃っているかのテスト用)
#[cfg(test)]
pub(crate) fn placeholders(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else { break };
        out.push(&after[..close]);
        rest = &after[close + 1..];
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// 埋め込みのある文言。`tr!(Msg::Foo, n = 3, verb)` のように `名前 = 式` か、
/// 同名の変数がある時は名前だけを並べる (format! の暗黙キャプチャと同じ書き味)
#[macro_export]
macro_rules! tr {
    ($msg:expr $(, $name:ident $(= $val:expr)?)* $(,)?) => {
        $crate::lang::fmt($msg, &[$((stringify!($name), $crate::tr!(@arg $name $(= $val)?))),*])
    };
    (@arg $name:ident = $val:expr) => { &$val as &dyn ::std::fmt::Display };
    (@arg $name:ident) => { &$name as &dyn ::std::fmt::Display };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_roundtrip() {
        for lang in Lang::ALL {
            assert_eq!(Lang::parse(lang.as_str()), Some(lang));
        }
        assert_eq!(Lang::parse("fr"), None);
    }

    #[test]
    fn placeholders_match_between_languages() {
        for msg in Msg::ALL {
            assert_eq!(
                placeholders(ja::text(*msg)),
                placeholders(en::text(*msg)),
                "{msg:?} のプレースホルダが ja/en で食い違う"
            );
        }
    }

    #[test]
    fn fmt_substitutes_named_args() {
        set(Lang::En);
        assert_eq!(
            tr!(Msg::GitStagedLines, lines = 3, verb = "stage"),
            "staged 3 lines"
        );
        let branch = "main";
        assert_eq!(tr!(Msg::BranchSwitched, branch), "switched to main");
    }

    #[test]
    fn next_cycles() {
        assert_eq!(Lang::Ja.next(1), Lang::En);
        assert_eq!(Lang::En.next(1), Lang::Ja);
        assert_eq!(Lang::Ja.next(-1), Lang::En);
    }
}
