//! クリップボードへの書き出し。新規依存を足さない方針 (CLAUDE.md) なので git と同じく
//! 外部コマンドの呼び出しで済ませ、コマンドが無い環境 (ssh 越し等) では OSC 52 に落とす。
//!
//! ローカルのコマンドを先に試すのは、OSC 52 が端末側の許可設定に左右され、拒否されても
//! こちらには何も返ってこない (無音で失敗する) ため。使った手段は呼び出し側が notice に
//! 出すので、貼り付けられなかった時にどちらの経路だったかが分かる。

use std::env;
use std::io::{self, Write};
use std::process::{Command, Stdio};

/// OSC 52 で送れる **base64 済みペイロード** の上限。1 度に受け取れる長さは端末ごとに違い
/// (xterm の既定は ~100KB)、超えると黙って捨てられる。無音で失敗するより「大きすぎて
/// 送れない」と言う方がましなので、切り詰めず拒否する
const OSC52_MAX_PAYLOAD: usize = 100 * 1024;

/// 試す順に (コマンド, 引数)
const COMMANDS: &[(&str, &[&str])] = &[
    ("pbcopy", &[]),
    ("wl-copy", &[]),
    ("xclip", &["-selection", "clipboard"]),
    ("xsel", &["--clipboard", "--input"]),
    // WSL から Windows 側のクリップボードへ
    ("clip.exe", &[]),
];

/// クリップボードへ書き出し、使った手段の名前を返す
pub fn copy(text: &str) -> Result<&'static str, String> {
    for (program, args) in COMMANDS {
        if !usable(program) {
            continue;
        }
        // 失敗理由は握り潰して次の手段へ進む (未インストールと実行失敗を区別しても
        // 打てる手は同じで、最後に OSC 52 の結果だけを返せば足りる)
        if run(program, args, text).is_ok() {
            return Ok(program);
        }
    }
    osc52(text).map(|()| "osc52")
}

// X / Wayland が無い環境で xclip・wl-copy を起動すると、接続先を探しに行くぶんだけ
// TUI が止まる (最悪ハングする)。ディスプレイが見えている時だけ試す
fn usable(program: &str) -> bool {
    match program {
        "wl-copy" => env::var_os("WAYLAND_DISPLAY").is_some(),
        "xclip" | "xsel" => env::var_os("DISPLAY").is_some(),
        _ => true,
    }
}

// stdin へ書いて drop (EOF) してから終了を待つ。git.rs の run_git_stdin と同じ作法
fn run(program: &str, args: &[&str], text: &str) -> Result<(), ()> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| ())?;
    let mut stdin = child.stdin.take().ok_or(())?;
    let written = stdin.write_all(text.as_bytes());
    drop(stdin);
    written.map_err(|_| ())?;
    match child.wait() {
        Ok(status) if status.success() => Ok(()),
        _ => Err(()),
    }
}

// OSC 52 は端末そのものへの指示なので、alternate screen の中身を汚さずに送れる。
// ratatui と同じ stdout に書くため、フレームの描画とは別に flush する
fn osc52(text: &str) -> Result<(), String> {
    // 端末へ実際に流れるのは base64 展開後の長さなので、生テキストではなくそちらで判定する。
    // base64 は 3 バイト → 4 文字と決まっているのでエンコード前に正確に見積もれる
    // (巨大なファイルを一度 base64 に起こしてから捨てる、という無駄も避けられる)
    let payload_len = encoded_len(text.len());
    if payload_len > OSC52_MAX_PAYLOAD {
        return Err(format!(
            "too large for the terminal clipboard ({} KB of base64 > {} KB)",
            payload_len / 1024,
            OSC52_MAX_PAYLOAD / 1024
        ));
    }
    let payload = base64(text.as_bytes());
    let mut out = io::stdout();
    // c = CLIPBOARD セレクション
    write!(out, "\x1b]52;c;{payload}\x07").map_err(|e| e.to_string())?;
    out.flush().map_err(|e| e.to_string())
}

// base64 済みの長さ。パディング込みなので 3 バイト単位に切り上げてから 4/3 倍する
fn encoded_len(len: usize) -> usize {
    len.div_ceil(3) * 4
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

// base64 だけのために依存を足さない (CLAUDE.md)。パディング込みの標準アルファベット
fn base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64[(triple >> 18) as usize & 0x3f] as char);
        out.push(B64[(triple >> 12) as usize & 0x3f] as char);
        out.push(if chunk.len() > 1 {
            B64[(triple >> 6) as usize & 0x3f] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[triple as usize & 0x3f] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{base64, encoded_len};

    #[test]
    fn base64_matches_rfc4648_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn base64_handles_multibyte_text() {
        assert_eq!(base64("あ".as_bytes()), "44GC");
    }

    // OSC 52 の上限判定はエンコード前にこの見積りだけで行うので、実際の出力と一致していないと
    // 「上限を超えたペイロードを黙って送る」に戻る
    #[test]
    fn encoded_len_matches_the_encoder() {
        for len in 0..40 {
            let input = vec![b'x'; len];
            assert_eq!(encoded_len(len), base64(&input).len(), "len = {len}");
        }
    }
}
