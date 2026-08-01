#!/bin/sh
# ソースが変わるたびに cargo preview (= --features preview) を描き直す。
#
#   scripts/preview-watch.sh            # view シーン
#   scripts/preview-watch.sh git log    # 複数シーンを並べる
#   scripts/preview-watch.sh all
#
# cargo-watch / bacon が入っていればそちらでも同じことができる:
#   cargo watch -x 'preview git --color'
# ここでは新しいツールを前提にしないため、内容のチェックサムを 1 秒ごとに見る形にしてある
# (mtime ではなく内容を見るので、touch や保存し直しだけでは再ビルドしない)
set -eu

cd "$(dirname "$0")/.."
scenes="${*:-view}"
previous=""

while :; do
    current=$(find src Cargo.toml -type f -exec cat {} + | cksum)
    if [ "$current" != "$previous" ]; then
        previous="$current"
        clear
        printf '\033[2m$ cargo preview %s\033[0m\n' "$scenes"
        # shellcheck disable=SC2086 # scenes は複数シーン名として分割したい
        cargo preview $scenes --color || true
    fi
    sleep 1
done
