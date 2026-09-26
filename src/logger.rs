// 診断用の永続ログ。TUI 実行中は stdout/stderr が画面そのもの (alternate screen + raw mode) なので、
// eprintln! で書くと描画が崩れて残らない。失敗経路の診断情報はここを通してファイルへだけ書き、
// UI への表示 (notice・Content::Error 等) は従来どおり呼び出し側が持つ — ログは「後から何が
// 起きたかを追える」ための追加情報で、UI の代わりにはしない。
//
// log クレート等の依存は足さない。やることは「閾値で落とす → 1 行に整形 → 追記」だけで、
// ファサードや複数出力先の抽象は要らないため。
//
// I/O の失敗 (書けないディレクトリ・HOME 不明・ディスク満杯) は TUI 中には一切表に出さない。
// その時点で出力を止め、端末を戻した後に main が `finish` の戻り値を 1 行だけ stderr に出す。

use std::ffi::OsStr;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// 既定の閾値。エラーと警告だけを残し、正常系の git の非ゼロ終了 (upstream が無い等) は
/// debug に置いて既定では書かない
const DEFAULT_LEVEL: Level = Level::Warn;
/// 1 ファイルの上限。超えたら `fv.log.1` へ 1 世代だけ退避する (診断用なので古い履歴は要らない)
const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// 1 メッセージの上限 (char 数)。stderr 丸ごと等を誤って渡しても 1 行が際限なく伸びないように
const MAX_MESSAGE_CHARS: usize = 500;
const REDACTED: &str = "[REDACTED]";
// GitHub のトークン接頭辞 (PAT / OAuth / App / refresh)。gh や git の stderr に紛れても残さない
const TOKEN_PREFIXES: [&str; 6] = ["github_pat_", "ghp_", "gho_", "ghu_", "ghs_", "ghr_"];

/// `Off` は閾値専用 (「何も書かない」)。メッセージ側に Off を渡す経路は公開しない
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Level {
    Off,
    Error,
    Warn,
    Info,
    Debug,
}

impl Level {
    /// `FV_LOG` の値。未知の値は None (呼び出し側が既定値に倒す)
    fn parse(value: &str) -> Option<Level> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" | "none" | "0" => Some(Level::Off),
            "error" => Some(Level::Error),
            "warn" | "warning" => Some(Level::Warn),
            "info" => Some(Level::Info),
            "debug" | "trace" => Some(Level::Debug),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Level::Off => "OFF",
            Level::Error => "ERROR",
            Level::Warn => "WARN",
            Level::Info => "INFO",
            Level::Debug => "DEBUG",
        }
    }
}

static LOGGER: OnceLock<Mutex<Logger>> = OnceLock::new();

/// 環境変数から閾値と出力先を決めて有効化する。TUI 起動前 (App::new より前) に 1 度だけ呼ぶ。
/// ファイルはここでは開かない — 何も起きなかった起動でログファイルを作らないため、
/// 最初の 1 行を書く時に初めて開く。呼ばれていない間 (テスト・--preview) は全て no-op
pub fn init() {
    let max = std::env::var("FV_LOG")
        .ok()
        .and_then(|v| Level::parse(&v))
        .unwrap_or(DEFAULT_LEVEL);
    let path = resolve_path(
        std::env::var_os("FV_LOG_FILE").as_deref(),
        std::env::var_os("XDG_STATE_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    );
    let _ = LOGGER.set(Mutex::new(Logger::new(max, path)));
}

/// 出力先: `FV_LOG_FILE` > `$XDG_STATE_HOME/fv/fv.log` > `~/.local/state/fv/fv.log`。
/// XDG の state は「再起動を跨いで残すがユーザーデータではないもの」(ログ・履歴) の置き場
fn resolve_path(
    override_file: Option<&OsStr>,
    xdg_state_home: Option<&OsStr>,
    home: Option<&OsStr>,
) -> Option<PathBuf> {
    if let Some(file) = override_file.filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(file));
    }
    // XDG Base Directory の規定どおり、相対パスの XDG_STATE_HOME は無効として扱う
    if let Some(dir) = xdg_state_home.filter(|v| !v.is_empty())
        && Path::new(dir).is_absolute()
    {
        return Some(Path::new(dir).join("fv").join("fv.log"));
    }
    let home = home.filter(|v| !v.is_empty())?;
    Some(
        Path::new(home)
            .join(".local")
            .join("state")
            .join("fv")
            .join("fv.log"),
    )
}

pub fn error(scope: &str, message: impl fmt::Display) {
    log(Level::Error, scope, message);
}

pub fn warn(scope: &str, message: impl fmt::Display) {
    log(Level::Warn, scope, message);
}

pub fn info(scope: &str, message: impl fmt::Display) {
    log(Level::Info, scope, message);
}

pub fn debug(scope: &str, message: impl fmt::Display) {
    log(Level::Debug, scope, message);
}

// message は Display で受け、閾値を越えた時だけ文字列化する (呼び出し側は format_args! を
// 渡せば、debug を捨てる既定設定で整形コストを払わない)
fn log(level: Level, scope: &str, message: impl fmt::Display) {
    let Some(cell) = LOGGER.get() else {
        return;
    };
    // 別スレッド (job) が書き込み中に panic して poison しても、ログは止めずに続ける
    let mut logger = cell.lock().unwrap_or_else(|e| e.into_inner());
    if !logger.enabled(level) {
        return;
    }
    logger.record(SystemTime::now(), level, scope, &message.to_string());
}

/// panic hook から呼ぶ。panic したのがロック保持中のスレッド自身だと lock() は二度と返らない
/// ため try_lock にし、取れなければ諦める (panic メッセージ自体は既定の hook が stderr に出す)
pub fn panic(info: &std::panic::PanicHookInfo<'_>) {
    let Some(cell) = LOGGER.get() else {
        return;
    };
    let mut logger = match cell.try_lock() {
        Ok(logger) => logger,
        Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => return,
    };
    if !logger.enabled(Level::Error) {
        return;
    }
    logger.record(SystemTime::now(), Level::Error, "panic", &info.to_string());
    logger.flush_repeats(SystemTime::now());
}

/// 終了時 (端末を戻した後) に呼ぶ。保留中の「繰り返し N 回」を書き出し、ログが書けなかった
/// ならその理由を返す — main はこれを stderr に 1 行出す (TUI 中には出せなかったフォールバック)
pub fn finish() -> Option<String> {
    let cell = LOGGER.get()?;
    let mut logger = cell.lock().unwrap_or_else(|e| e.into_inner());
    logger.flush_repeats(SystemTime::now());
    logger.failure_report()
}

/// git / gh の引数からログに出してよい部分 (先頭 `words` 個のサブコマンド名) だけを取り出す。
/// 残りの引数にはパス・ブランチ名・コミット範囲が入るので書かない
pub fn command_label<S: AsRef<OsStr>>(args: &[S], words: usize) -> String {
    args.iter()
        .take(words)
        .map(|a| a.as_ref().to_string_lossy())
        .take_while(|a| !a.starts_with('-'))
        .collect::<Vec<_>>()
        .join(" ")
}

enum Sink {
    /// まだ 1 行も書いていない (ファイルは開いていない)
    Pending,
    Open {
        file: File,
        size: u64,
    },
    /// 開けない・書けなかった。以後は試さない (失敗する I/O を毎回繰り返さない)
    Disabled,
}

struct Logger {
    max: Level,
    max_bytes: u64,
    path: Option<PathBuf>,
    sink: Sink,
    pid: u32,
    /// 直前に書いた (level, scope, message)。同じものが続く間は書かずに数えるだけにする —
    /// git が無い環境の rescan や監視の失敗は同じ行を延々と繰り返すため
    last: Option<(Level, String, String)>,
    repeats: u64,
    failure: Option<String>,
    dropped: u64,
}

impl Logger {
    fn new(max: Level, path: Option<PathBuf>) -> Self {
        Self {
            max,
            max_bytes: MAX_FILE_BYTES,
            path,
            sink: Sink::Pending,
            pid: std::process::id(),
            last: None,
            repeats: 0,
            failure: None,
            dropped: 0,
        }
    }

    fn enabled(&self, level: Level) -> bool {
        level != Level::Off && level <= self.max
    }

    fn record(&mut self, now: SystemTime, level: Level, scope: &str, message: &str) {
        let message = sanitize(&redact(message));
        if let Some((l, s, m)) = &self.last
            && *l == level
            && s == scope
            && *m == message
        {
            self.repeats += 1;
            return;
        }
        self.flush_repeats(now);
        let line = format_line(now, self.pid, level, scope, &message);
        self.write_line(&line);
        self.last = Some((level, scope.to_string(), message));
    }

    fn flush_repeats(&mut self, now: SystemTime) {
        if self.repeats == 0 {
            return;
        }
        let Some((level, scope, _)) = &self.last else {
            return;
        };
        let line = format_line(
            now,
            self.pid,
            *level,
            scope,
            &format!("(previous message repeated {} times)", self.repeats),
        );
        self.repeats = 0;
        self.write_line(&line);
    }

    fn write_line(&mut self, line: &str) {
        if let Sink::Open { size, .. } = &self.sink
            && *size + line.len() as u64 > self.max_bytes
        {
            // 走行中に上限へ達した。閉じて (退避は open が行う) 新しいファイルで続ける
            self.sink = Sink::Pending;
        }
        if matches!(self.sink, Sink::Pending) {
            self.sink = self.open(line.len() as u64);
        }
        let Sink::Open { file, size } = &mut self.sink else {
            self.dropped += 1;
            return;
        };
        match file.write_all(line.as_bytes()) {
            Ok(()) => *size += line.len() as u64,
            Err(e) => {
                self.fail(format!("cannot write {}: {e}", self.display_path()));
                self.dropped += 1;
            }
        }
    }

    fn open(&mut self, incoming: u64) -> Sink {
        let Some(path) = self.path.clone() else {
            self.fail("cannot determine the log location (HOME is not set)".to_string());
            return Sink::Disabled;
        };
        if let Some(dir) = path.parent()
            && !dir.as_os_str().is_empty()
            && let Err(e) = fs::create_dir_all(dir)
        {
            self.fail(format!("cannot create {}: {e}", dir.display()));
            return Sink::Disabled;
        }
        rotate_if_full(&path, incoming, self.max_bytes);
        match open_append(&path) {
            Ok(file) => {
                let size = file.metadata().map(|m| m.len()).unwrap_or(0);
                Sink::Open { file, size }
            }
            Err(e) => {
                self.fail(format!("cannot open {}: {e}", path.display()));
                Sink::Disabled
            }
        }
    }

    fn fail(&mut self, reason: String) {
        self.sink = Sink::Disabled;
        if self.failure.is_none() {
            self.failure = Some(reason);
        }
    }

    fn display_path(&self) -> String {
        self.path
            .as_deref()
            .map(|p| p.display().to_string())
            .unwrap_or_default()
    }

    fn failure_report(&self) -> Option<String> {
        let reason = self.failure.as_ref()?;
        Some(format!(
            "fv: diagnostic log disabled: {reason} ({} message(s) dropped)",
            self.dropped
        ))
    }
}

// 大きさは開き直す時点の実ファイルで測る (同じファイルへ別の fv プロセスも追記しうるため)
fn rotate_if_full(path: &Path, incoming: u64, max_bytes: u64) {
    let Ok(meta) = fs::metadata(path) else {
        return;
    };
    if meta.len() == 0 || meta.len() + incoming <= max_bytes {
        return;
    }
    let mut old = path.as_os_str().to_os_string();
    old.push(".1");
    // 退避に失敗しても追記は続ける (上限を少し越えるだけで、診断情報を失うよりよい)
    let _ = fs::rename(path, PathBuf::from(old));
}

// パス・エラー理由だけとはいえ他ユーザーに読ませるものではないので、新規作成時は 0600 にする
fn open_append(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn format_line(now: SystemTime, pid: u32, level: Level, scope: &str, message: &str) -> String {
    format!(
        "{} {pid} {:<5} {scope}: {message}\n",
        timestamp(now),
        level.as_str()
    )
}

/// UTC の ISO 8601 (秒精度)。chrono 等を足さないため日付計算は自前で行う
fn timestamp(now: SystemTime) -> String {
    let secs = now
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (year, month, day) = civil_from_days((secs / 86_400) as i64);
    let rem = secs % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

// 1970-01-01 からの日数 → (年, 月, 日)。Howard Hinnant の days_from_civil の逆変換
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// 1 メッセージ = 1 行を保つ: 改行・ESC 等の制御文字を空白にし (端末でログを cat した時に
/// エスケープシーケンスが効かないようにも)、長すぎる分は切り詰める
fn sanitize(message: &str) -> String {
    let mut out = String::with_capacity(message.len().min(MAX_MESSAGE_CHARS + 1));
    for (i, c) in message.chars().enumerate() {
        if i == MAX_MESSAGE_CHARS {
            out.push('…');
            break;
        }
        out.push(if c.is_control() { ' ' } else { c });
    }
    out
}

/// 認証情報になりうる部分を伏せる。呼び出し側がそもそも本文・引数を渡さないのが第一の防御で、
/// これは git/gh の stderr に紛れ込むもの (URL の userinfo・GitHub トークン) への保険
fn redact(message: &str) -> String {
    redact_tokens(&redact_url_userinfo(message))
}

// `scheme://userinfo@host` の userinfo を伏せる。push/fetch の失敗は remote URL を
// そのまま stderr に出すため
fn redact_url_userinfo(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    let mut rest = message;
    while let Some(pos) = rest.find("://") {
        let (head, tail) = rest.split_at(pos + 3);
        out.push_str(head);
        let authority_end = tail
            .find(|c: char| c == '/' || c.is_whitespace() || c == '\'' || c == '"')
            .unwrap_or(tail.len());
        let authority = &tail[..authority_end];
        match authority.rfind('@') {
            Some(at) => {
                out.push_str(REDACTED);
                out.push_str(&authority[at..]);
            }
            None => out.push_str(authority),
        }
        rest = &tail[authority_end..];
    }
    out.push_str(rest);
    out
}

fn redact_tokens(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    let mut rest = message;
    'scan: while !rest.is_empty() {
        for prefix in TOKEN_PREFIXES {
            // 単語の途中 (例: "foo_ghp_") は対象外にし、トークンの頭だけを拾う
            let at_word_start = out
                .chars()
                .last()
                .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'));
            if at_word_start && rest.starts_with(prefix) {
                let len = rest[prefix.len()..]
                    .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                    .unwrap_or(rest.len() - prefix.len());
                if len > 0 {
                    out.push_str(REDACTED);
                    rest = &rest[prefix.len() + len..];
                    continue 'scan;
                }
            }
        }
        let c = rest.chars().next().unwrap_or_default();
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    // 資格情報らしき文字列をソースに直書きしない (シークレットスキャナの誤検知を避ける)
    const CRED: &str = concat!("user", ":", "pw");

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fv-logger-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn parses_levels() {
        assert_eq!(Level::parse("off"), Some(Level::Off));
        assert_eq!(Level::parse(" ERROR "), Some(Level::Error));
        assert_eq!(Level::parse("warning"), Some(Level::Warn));
        assert_eq!(Level::parse("info"), Some(Level::Info));
        assert_eq!(Level::parse("Debug"), Some(Level::Debug));
        assert_eq!(Level::parse("loud"), None);
    }

    #[test]
    fn threshold_filters_levels() {
        let logger = Logger::new(Level::Warn, None);
        assert!(logger.enabled(Level::Error));
        assert!(logger.enabled(Level::Warn));
        assert!(!logger.enabled(Level::Info));
        assert!(!logger.enabled(Level::Debug));
        let off = Logger::new(Level::Off, None);
        assert!(!off.enabled(Level::Error));
        assert!(!off.enabled(Level::Off));
    }

    #[test]
    fn resolves_path_in_priority_order() {
        let os = |s: &'static str| Some(OsStr::new(s));
        assert_eq!(
            resolve_path(os("/tmp/x.log"), os("/state"), os("/home/u")),
            Some(PathBuf::from("/tmp/x.log"))
        );
        assert_eq!(
            resolve_path(None, os("/state"), os("/home/u")),
            Some(PathBuf::from("/state/fv/fv.log"))
        );
        // 空・相対の XDG_STATE_HOME は無視して HOME にフォールバックする
        assert_eq!(
            resolve_path(os(""), os("relative"), os("/home/u")),
            Some(PathBuf::from("/home/u/.local/state/fv/fv.log"))
        );
        assert_eq!(resolve_path(None, None, None), None);
    }

    #[test]
    fn formats_utc_timestamps() {
        assert_eq!(timestamp(at(0)), "1970-01-01T00:00:00Z");
        assert_eq!(timestamp(at(951_782_400)), "2000-02-29T00:00:00Z");
        assert_eq!(timestamp(at(1_790_396_392)), "2026-09-26T04:19:52Z");
        assert_eq!(timestamp(at(4_102_444_799)), "2099-12-31T23:59:59Z");
    }

    #[test]
    fn formats_a_single_line() {
        assert_eq!(
            format_line(at(0), 42, Level::Warn, "git", "boom"),
            "1970-01-01T00:00:00Z 42 WARN  git: boom\n"
        );
    }

    #[test]
    fn sanitizes_control_chars_and_length() {
        assert_eq!(sanitize("a\nb\r\tc\x1b[31m"), "a b  c [31m");
        let long = "x".repeat(MAX_MESSAGE_CHARS + 10);
        let cut = sanitize(&long);
        assert_eq!(cut.chars().count(), MAX_MESSAGE_CHARS + 1);
        assert!(cut.ends_with('…'));
        assert_eq!(sanitize("日本語"), "日本語");
    }

    #[test]
    fn redacts_url_credentials() {
        assert_eq!(
            redact(&format!(
                "fatal: unable to access 'https://{CRED}@github.com/o/r.git/': 403"
            )),
            "fatal: unable to access 'https://[REDACTED]@github.com/o/r.git/': 403"
        );
        assert_eq!(
            redact(&format!("https://{CRED}@github.com and ssh://git@host/r")),
            "https://[REDACTED]@github.com and ssh://[REDACTED]@host/r"
        );
        assert_eq!(
            redact("remote https://github.com/o/r"),
            "remote https://github.com/o/r"
        );
    }

    #[test]
    fn redacts_github_tokens() {
        assert_eq!(
            redact("token ghp_ABCdef123 rejected"),
            "token [REDACTED] rejected"
        );
        assert_eq!(redact("github_pat_11AA_bb"), "[REDACTED]");
        // 単語の途中・接頭辞だけのものは触らない
        assert_eq!(redact("my_ghp_x ghp_"), "my_ghp_x ghp_");
        assert_eq!(redact("日本語 gho_x"), "日本語 [REDACTED]");
    }

    #[test]
    fn labels_only_subcommands() {
        assert_eq!(
            command_label(&["switch", "--", "secret-branch"], 1),
            "switch"
        );
        assert_eq!(
            command_label(&["issue", "view", "12", "--web"], 2),
            "issue view"
        );
        assert_eq!(command_label(&["pr", "--help"], 2), "pr");
        let empty: [&str; 0] = [];
        assert_eq!(command_label(&empty, 1), "");
    }

    #[test]
    fn writes_lazily_and_collapses_repeats() {
        let dir = temp_dir("repeat");
        let path = dir.join("nested").join("fv.log");
        let mut logger = Logger::new(Level::Warn, Some(path.clone()));
        // 1 行も書くまではファイルを作らない
        assert!(!path.exists());
        logger.record(at(0), Level::Warn, "git", "boom");
        logger.record(at(1), Level::Warn, "git", "boom");
        logger.record(at(2), Level::Warn, "git", "boom");
        logger.record(at(3), Level::Error, "watch", "other");
        logger.record(at(4), Level::Error, "watch", "other");
        logger.flush_repeats(at(5));
        let text = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 4, "{text}");
        assert!(lines[0].ends_with("WARN  git: boom"));
        assert!(lines[1].ends_with("WARN  git: (previous message repeated 2 times)"));
        assert!(lines[2].ends_with("ERROR watch: other"));
        assert!(lines[3].ends_with("ERROR watch: (previous message repeated 1 times)"));
        assert!(logger.failure_report().is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotates_large_files() {
        let dir = temp_dir("rotate");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("fv.log");
        fs::write(&path, vec![b'x'; MAX_FILE_BYTES as usize]).unwrap();
        let mut logger = Logger::new(Level::Warn, Some(path.clone()));
        logger.record(at(0), Level::Warn, "git", "fresh");
        assert_eq!(
            fs::metadata(dir.join("fv.log.1")).unwrap().len(),
            MAX_FILE_BYTES
        );
        assert!(fs::read_to_string(&path).unwrap().ends_with("git: fresh\n"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotates_while_running() {
        let dir = temp_dir("rotate-running");
        let path = dir.join("fv.log");
        let mut logger = Logger::new(Level::Warn, Some(path.clone()));
        let line_len = format_line(at(0), logger.pid, Level::Warn, "git", "m0").len() as u64;
        logger.max_bytes = line_len * 2;
        for i in 0..3 {
            logger.record(at(i), Level::Warn, "git", &format!("m{i}"));
        }
        let old = fs::read_to_string(dir.join("fv.log.1")).unwrap();
        let new = fs::read_to_string(&path).unwrap();
        assert_eq!(old.lines().count(), 2, "{old}");
        assert!(new.ends_with("git: m2\n"), "{new}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn reports_failure_instead_of_writing_to_the_terminal() {
        let dir = temp_dir("fail");
        fs::create_dir_all(&dir).unwrap();
        // 親ディレクトリの位置に通常ファイルがあるので作成できない
        let blocker = dir.join("blocker");
        fs::write(&blocker, "").unwrap();
        let mut logger = Logger::new(Level::Warn, Some(blocker.join("fv.log")));
        logger.record(at(0), Level::Warn, "git", "one");
        logger.record(at(1), Level::Error, "git", "two");
        let report = logger.failure_report().unwrap();
        assert!(report.contains("cannot create"), "{report}");
        assert!(report.contains("2 message(s) dropped"), "{report}");

        let mut homeless = Logger::new(Level::Warn, None);
        homeless.record(at(0), Level::Warn, "git", "one");
        assert!(homeless.failure_report().unwrap().contains("HOME"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn redacts_before_writing() {
        let dir = temp_dir("redact");
        let path = dir.join("fv.log");
        let mut logger = Logger::new(Level::Warn, Some(path.clone()));
        logger.record(
            at(0),
            Level::Warn,
            "git",
            &format!("push failed: https://{CRED}@github.com/o/r\nsecond line"),
        );
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains(CRED), "{text}");
        assert_eq!(text.lines().count(), 1, "{text}");
        let _ = fs::remove_dir_all(&dir);
    }
}
