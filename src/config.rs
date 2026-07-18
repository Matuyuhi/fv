use std::env;
use std::fs;
use std::io;
use std::path::PathBuf;

// 設定画面 (s キー) で変更した値の永続化。toml/serde 等は依存に足さず、
// `key = value` の独自最小フォーマットで自前パースする
#[derive(Clone)]
pub struct Config {
    pub show_hidden: bool,
    pub icons: bool,
    pub wrap_default: bool,
    pub theme: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            show_hidden: false,
            icons: false,
            wrap_default: false,
            theme: "base16-ocean.dark".to_string(),
        }
    }
}

impl Config {
    /// 設定ファイルが無い/読めない場合は None を返す。呼び出し側で
    /// CLI 引数や既存のデフォルト判定にフォールバックさせるため Option にしている
    pub fn load() -> Option<Config> {
        let path = config_path()?;
        let text = fs::read_to_string(path).ok()?;
        let mut config = Config::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "show_hidden" => config.show_hidden = value == "true",
                "icons" => config.icons = value == "true",
                "wrap_default" => config.wrap_default = value == "true",
                "theme" => config.theme = value.to_string(),
                _ => {}
            }
        }
        Some(config)
    }

    pub fn save(&self) -> io::Result<()> {
        // HOME が取れない環境では何もしない (エラーにはしない)
        let Some(path) = config_path() else {
            return Ok(());
        };
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let body = format!(
            "show_hidden = {}\nicons = {}\nwrap_default = {}\ntheme = {}\n",
            self.show_hidden, self.icons, self.wrap_default, self.theme
        );
        fs::write(path, body)
    }
}

fn config_path() -> Option<PathBuf> {
    if let Ok(xdg) = env::var("XDG_CONFIG_HOME")
        && !xdg.is_empty()
    {
        return Some(PathBuf::from(xdg).join("fv").join("config"));
    }
    let home = env::var("HOME").ok()?;
    Some(
        PathBuf::from(home)
            .join(".config")
            .join("fv")
            .join("config"),
    )
}
