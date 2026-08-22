use std::env;
use std::fs;
use std::io;
use std::path::PathBuf;

// 設定画面 (s キー) で変更した値の永続化。toml/serde 等は依存に足さず、
// `key = value` の独自最小フォーマットで自前パースする
#[derive(Clone)]
pub struct Config {
    pub show_hidden: bool,
    /// .gitignore 等で無視されるファイルもツリー・Finder に出す
    pub show_ignored: bool,
    pub icons: bool,
    pub wrap_default: bool,
    pub theme: String,
    /// 左ペイン (ツリー) が画面幅に占める割合。桁数でなく割合で持つのは
    /// 端末サイズが変わっても見た目の配分を保つため
    pub split_ratio: f32,
    /// GitHub モード (ヘッダタブ) の有効化。CLI の `--github` はこの値を書き換えず、
    /// App 側でその起動限りの上乗せとして扱う (App::new / toggle_github 参照)
    pub github: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            show_hidden: false,
            show_ignored: false,
            icons: false,
            wrap_default: false,
            theme: "base16-ocean.dark".to_string(),
            split_ratio: 0.30,
            github: false,
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
                "show_ignored" => config.show_ignored = value == "true",
                "icons" => config.icons = value == "true",
                "wrap_default" => config.wrap_default = value == "true",
                "theme" => config.theme = value.to_string(),
                // 壊れた値は既定のまま無視する (割合の妥当な範囲への丸めは App 側の clamp に任せる)
                "split_ratio" => {
                    if let Ok(ratio) = value.parse::<f32>()
                        && ratio.is_finite()
                    {
                        config.split_ratio = ratio;
                    }
                }
                "github" => config.github = value == "true",
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
            "show_hidden = {}\nshow_ignored = {}\nicons = {}\nwrap_default = {}\ntheme = {}\nsplit_ratio = {:.3}\ngithub = {}\n",
            self.show_hidden,
            self.show_ignored,
            self.icons,
            self.wrap_default,
            self.theme,
            self.split_ratio,
            self.github
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
