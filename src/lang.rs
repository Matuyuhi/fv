use std::sync::atomic::{AtomicU8, Ordering};

// UI 文言の言語。プロセス全体で 1 つの値を static に持つのは、描画関数 (自分の状態しか
// 受け取らない設計) と背景スレッド (gh/git の失敗メッセージ) の両方から同じ値を読むためで、
// 引数で配って回ると全ての draw_* と notice の組み立てに引数の変更が及ぶ。
// 文言そのものは各呼び出し側に `t("日本語", "English")` の対で置き、翻訳表 (キー → 文字列)
// は持たない — 対で書く以上、片方だけ書き忘れた文言は型上作れない
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

/// 固定文言。引数の対がそのまま翻訳表になる
pub fn t(ja: &'static str, en: &'static str) -> &'static str {
    match current() {
        Lang::Ja => ja,
        Lang::En => en,
    }
}

/// 埋め込みのある文言。選ばれた側だけ format! する
/// (`tr!("hunk {n} を stage しました", "staged hunk {n}")` のように書式文字列を対で渡す)
#[macro_export]
macro_rules! tr {
    ($ja:literal, $en:literal $($rest:tt)*) => {
        match $crate::lang::current() {
            $crate::lang::Lang::Ja => format!($ja $($rest)*),
            $crate::lang::Lang::En => format!($en $($rest)*),
        }
    };
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
    fn next_cycles() {
        assert_eq!(Lang::Ja.next(1), Lang::En);
        assert_eq!(Lang::En.next(1), Lang::Ja);
        assert_eq!(Lang::Ja.next(-1), Lang::En);
    }
}
