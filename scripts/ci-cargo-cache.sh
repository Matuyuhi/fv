#!/bin/sh
# actions/cache に渡す cargo キャッシュの「鍵」と、保存直前の「刈り込み」(CI 専用)。
# ci.yml / perf.yml / release.yml の 3 本が同じ鍵・同じ刈り込みを使うために 1 箇所に置く。
#
#   scripts/ci-cargo-cache.sh key    # 鍵の部品 (rustc=… / deps=…) を $GITHUB_OUTPUT の書式で stdout へ
#   scripts/ci-cargo-cache.sh prune  # target/ から「毎回作り直すもの」を落とす
#
# リポジトリのキャッシュ上限 (10GB) を食い潰さないための方針は 2 つ:
#
# 1. 鍵は「依存 (Cargo.lock) と rustc の版」が変わった時だけ変わる。Cargo.lock には fv 自身の
#    version 行が含まれ release (bump) のたびに変わるので、version 行を除いてハッシュする
#    (依存の版が変われば checksum 行に必ず出るので取りこぼさない)。rustc の版を鍵に含める
#    のは、`rustup update stable` で toolchain が上がると target/ の中身が丸ごと使えなくなる
#    のに鍵が同じままだと「毎回フルビルドするのに保存し直されない」状態で固定されるため。
#    **restore-keys は付けない (完全一致だけ復元する)**。近い鍵から復元すると、古い rustc や
#    古い版の依存の成果物を cargo は捨てないので、新しい成果物と同居したまま新しい鍵で
#    保存され、鍵が変わるたびに太る。成果物名 (deps/lib<crate>-<hash>) から版を引けないため
#    保存前に古い分だけを選んで消すこともできない。鍵が変わった時に 1 度だけ払うコールド
#    ビルドは実測で 1 分強 (依存の総量が小さい) なので、部分復元の得より確実さを取る。
#
# 2. 保存する中身は「依存のビルド成果物」だけに絞る。fv 自身の成果物 (lib・バイナリ・
#    テスト実行ファイル・フィンガープリント) はコミットごとに必ず作り直されるので、残しても
#    次の実行で使われない上に、コミットごとに違う内容になる。incremental は debug ビルドで
#    数百 MB になるが CI では毎回ソースが違うので効かない (CARGO_INCREMENTAL=0 で
#    そもそも作らせない。ここで消すのは保険)。同じ鍵でも中身がコミットごとに膨らむのを止め、
#    1 つのキャッシュの大きさを依存の総量で頭打ちにする。
set -eu

case "${1:-}" in
    key)
        # rustc の版は commit hash まで含めた `rustc -V` 全体で取る (nightly や同じ番号の
        # 再ビルドを区別するため)。空白は鍵に使えないので潰す
        rustc_ver=$(rustc -V | tr -cs 'A-Za-z0-9.' '-' | sed 's/^-*//; s/-*$//')
        lock_hash=$(grep -v '^version = "' Cargo.lock | shasum -a 256 | cut -d' ' -f1)
        # 2 つに分けて出すのは、restore-keys を「同じ rustc の範囲」で止めるため
        echo "rustc=${rustc_ver}"
        echo "deps=${lock_hash}"
        ;;
    prune)
        [ -d target ] || exit 0
        before=$(du -sm target | cut -f1)
        # profile ディレクトリを深さを問わず拾う (target/debug, target/<triple>/release,
        # target/perf-base/release ...)。目印は deps/ を持つこと
        find target -type d -name deps -prune | while read -r deps; do
            prof=$(dirname "$deps")
            rm -rf "$prof/incremental" "$prof/examples"
            rm -f "$prof/fv" "$prof/fv.d" "$prof/fv.exe" "$prof/fv.pdb"
            rm -rf "$deps"/fv-* "$deps"/libfv-* "$prof"/.fingerprint/fv-*
        done
        rm -rf target/tmp
        after=$(du -sm target | cut -f1)
        echo "pruned target/: ${before}MB -> ${after}MB"
        ;;
    *)
        echo "usage: $0 key|prune" >&2
        exit 2
        ;;
esac
