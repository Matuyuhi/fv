//! Nerd Font グリフのテーブル。コードポイントは Nerd Font v3 でも移動しない安定領域
//! (Devicons e700-, Seti e5fa-, Font Awesome f000-, Font Logos f300-) のみ使う。
//! Material 系 (旧 f500-fd46) は v3 で U+F0001 以降へ移動したため使わない。

pub(super) fn dir_icon(expanded: bool) -> &'static str {
    if expanded {
        "\u{f07c}" // fa-folder-open
    } else {
        "\u{f07b}" // fa-folder
    }
}

pub(super) fn file_icon(name: &str) -> &'static str {
    if let Some(icon) = special_name_icon(name) {
        return icon;
    }
    let ext = std::path::Path::new(name)
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase());
    match ext.as_deref() {
        Some("rs") => "\u{e7a8}",                                     // dev-rust
        Some("go") => "\u{e627}",                                     // seti-go
        Some("py") => "\u{e73c}",                                     // dev-python
        Some("js" | "mjs" | "cjs") => "\u{e74e}",                     // dev-javascript
        Some("ts") => "\u{e628}",                                     // seti-typescript
        Some("jsx" | "tsx") => "\u{e7ba}",                            // dev-react
        Some("kt" | "kts") => "\u{e634}",                             // seti-kotlin
        Some("swift") => "\u{e755}",                                  // dev-swift
        Some("java") => "\u{e738}",                                   // dev-java
        Some("rb") => "\u{e739}",                                     // dev-ruby
        Some("c" | "h") => "\u{e61e}",                                // seti-c
        Some("cpp" | "cc" | "hpp" | "hh") => "\u{e61d}",              // seti-cpp
        Some("lua") => "\u{e620}",                                    // seti-lua
        Some("vim") => "\u{e62b}",                                    // seti-vim
        Some("md" | "markdown") => "\u{e73e}",                        // dev-markdown
        Some("json") => "\u{e60b}",                                   // seti-json
        Some("yaml" | "yml" | "toml" | "ini" | "conf") => "\u{e615}", // seti-config
        Some("html" | "htm") => "\u{e736}",                           // dev-html5
        Some("css" | "scss" | "sass") => "\u{e749}",                  // dev-css3
        Some("sh" | "bash" | "zsh" | "fish") => "\u{e795}",           // dev-terminal
        Some("sql" | "db" | "sqlite") => "\u{e706}",                  // dev-database
        Some("lock") => "\u{f023}",                                   // fa-lock
        Some("txt") => "\u{f15c}",                                    // fa-file-text
        Some("pdf") => "\u{f1c1}",                                    // fa-file-pdf
        Some("png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "ico" | "bmp") => "\u{f03e}", // fa-image
        Some("zip" | "tar" | "gz" | "bz2" | "xz" | "7z") => "\u{f1c6}", // fa-file-archive
        _ => "\u{f016}",                                                // fa-file-o
    }
}

fn special_name_icon(name: &str) -> Option<&'static str> {
    match name {
        ".gitignore" | ".gitattributes" | ".gitmodules" => Some("\u{e702}"), // dev-git
        _ if name == "Dockerfile" || name.starts_with("docker-compose") => Some("\u{f308}"), // linux-docker
        _ if name.starts_with("LICENSE") => Some("\u{f02d}"), // fa-book
        _ => None,
    }
}
