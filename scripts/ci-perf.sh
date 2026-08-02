#!/bin/sh
# PR に「この変更で 1 打鍵あたりのコストがどう動いたか」を 1 個のコメントとして出す (CI 専用)。
#
# `fv --perf` (src/preview/perf.rs) を **base と head の両方で、同じ runner の上で連続して**
# 走らせて比べる。GitHub の runner は同居する他のジョブで絶対値がぶれるため、単独の数字に
# 意味を持たせず 2 点の比だけを見る、という前提でこの形にしてある。
#
# **差分があっても PR は落とさない**。UI スナップショット差分 (ci-ui-diff-comment.sh) と同じ
# 方針で、赤い × は「見せる」目的に何も足さず作者に対応を強いるだけになる。閾値を超えた
# ときだけ ::warning:: を出して気づけるようにする。
#
# base 側のビルドは git worktree で別ディレクトリに出す。同じ作業ツリーを checkout し直すと、
# 実行中のこのスクリプト自身が書き換わる (この PR のように base にまだ存在しない場合は消える)。
#
# コメントは毎回追加せず、隠しマーカーで自分の前回コメントを探して編集する。gh は runner
# 同梱のものを使い、Action は増やさない (このリポジトリの CI は checkout / cache のみ)。
set -eu

MARKER='<!-- fv:perf -->'
# これを超えて悪化した行があれば ::warning:: を出す。runner のぶれ (実測で数 %) より
# 十分大きく、かつ「1 打鍵が体感で重くなる」より前に気づける値
WARN_PCT=20

PERF_CMD="cargo run --release --quiet --features preview -- --perf"

# base 側は target/ を head と分ける。cargo のフィンガープリントは同じパッケージを
# ソースの場所で区別しないため、同じ target/ を使うと base のビルドが head のバイナリを
# 上書きし、しかも次に head を build しても「新しい」と判定されて base の実行ファイルを
# 測ってしまう (実際に踏んだ)。依存の再ビルドぶんは遅くなるが、間違った数字よりましで、
# target/ 配下なので既存のキャッシュ設定にそのまま乗る。
# サブシェルの中で $PWD を見ると worktree 側になるので、絶対パスはここで確定させておく
BASE_TARGET_DIR="$PWD/target/perf-base"

head_tsv=$(mktemp)
base_tsv=$(mktemp)
head_sha=$(git rev-parse HEAD)

echo "measuring head..."
$PERF_CMD >"$head_tsv"
cat "$head_tsv"

# $work の中で base を測る。空でない TSV が出れば成功
measure_base() {
    (cd "$work" && CARGO_TARGET_DIR="$BASE_TARGET_DIR" $PERF_CMD) >"$base_tsv" 2>/dev/null &&
        [ -s "$base_tsv" ]
}

have_base=0
grafted=0
if git fetch --no-tags --depth=1 origin "${GITHUB_BASE_REF:-}" >/dev/null 2>&1; then
    work=$(mktemp -d)
    if git worktree add --detach "$work" FETCH_HEAD >/dev/null 2>&1; then
        echo "measuring base (${GITHUB_BASE_REF})..."
        if measure_base; then
            have_base=1
        else
            # base にこの計測がまだ無い (これを追加した PR) / 古い形の場合。head 側の
            # 計測コードだけを base のツリーへ移植して測り直す — アプリのコードは base の
            # まま、ものさしだけ head に揃えることで比較が成立する。計測自体を変える PR で
            # 「base と head で違うものさしを比べる」のを防ぐ意味もある。
            # base が古すぎて head の preview/ がコンパイルできない場合は素直に諦める
            echo "base にこの計測が無い/古いので、head 側の計測コードを移植して測り直します"
            if git -C "$work" checkout "$head_sha" -- src/preview >/dev/null 2>&1 &&
                measure_base; then
                have_base=1
                grafted=1
            else
                echo "base の計測を実行できませんでした。head の値だけ出します"
            fi
        fi
        if [ "$have_base" -eq 1 ]; then
            cat "$base_tsv"
        fi
        git worktree remove --force "$work" >/dev/null 2>&1 || true
    fi
fi

if [ "${GITHUB_EVENT_NAME:-}" != "pull_request" ]; then
    echo "not a pull request; skip comment"
    exit 0
fi

body=$(mktemp)
printf '%s\n' "$MARKER" >>"$body"
printf '### 速度チェック\n\n' >>"$body"

if [ "$have_base" -eq 1 ]; then
    printf '1 打鍵 (キー入力 → 再描画 1 回) あたりの所要時間。小さいほど速い。\n\n' >>"$body"
    # ヘッダは awk の外で出す。中で出そうとすると「1 行目は # のコメント行なので
    # skip される」規則と順序が絡んで落としやすい (実際に落として気づいた)
    printf '| ケース | ops | base | head | 差分 |\n| --- | --: | --: | --: | --: |\n' >>"$body"
    worst=$(awk -F'\t' -v out="$body" '
        /^#/ { next }
        # 1 つ目のファイル = base
        FNR == NR { base[$1] = $4; next }
        {
            head_v = $4 + 0
            if (!($1 in base)) {
                printf "| `%s` | %s | — | %.3f ms | (base に無い) |\n", $1, $2, head_v >> out
                next
            }
            base_v = base[$1] + 0
            delta = base_v > 0 ? (head_v - base_v) / base_v * 100 : 0
            mark = delta > 0 ? "+" : ""
            printf "| `%s` | %s | %.3f ms | %.3f ms | %s%.1f%% |\n", $1, $2, base_v, head_v, mark, delta >> out
            if (delta > max) { max = delta; name = $1 }
        }
        END { printf "%s\t%.1f\n", name, max + 0 }
    ' "$base_tsv" "$head_tsv")
    worst_name=${worst%%	*}
    worst_pct=${worst##*	}
    printf '\n_同じ runner で連続して測った 2 点の比です。実測で数 %% はぶれるので、' >>"$body"
    printf 'それを超えた行だけ見てください。この結果で PR は落としません。_\n' >>"$body"
    if [ "$grafted" -eq 1 ]; then
        printf '\n_base 側にはこの計測がまだ無いため、**head の計測コードだけを base のツリーへ移植**して測っています' >>"$body"
        printf ' (アプリのコードは base のまま)。_\n' >>"$body"
    fi
    # awk の比較は文字列になりうるので数値として比べ直す
    if [ "$(awk -v a="$worst_pct" -v b="$WARN_PCT" 'BEGIN { print (a > b) ? 1 : 0 }')" = "1" ]; then
        echo "::warning::${worst_name} が base より ${worst_pct}% 遅くなっています"
    fi
else
    printf 'base ではこの計測を実行できなかったため (この PR で追加された等)、head の値だけ出します。\n\n' >>"$body"
    printf '| ケース | ops | head |\n| --- | --: | --: |\n' >>"$body"
    awk -F'\t' -v out="$body" '
        /^#/ { next }
        { printf "| `%s` | %s | %.3f ms |\n", $1, $2, $4 + 0 >> out }
    ' "$head_tsv"
fi

existing=$(gh api "repos/${GITHUB_REPOSITORY}/issues/${PR_NUMBER}/comments" --paginate \
    --jq "map(select(.body | startswith(\"${MARKER}\"))) | .[-1].id // empty" 2>/dev/null || true)

# fork からの PR では GITHUB_TOKEN が読み取り専用でコメントできない。
# それだけで CI を落とす理由は無いので、失敗しても警告に留める
if [ -n "$existing" ]; then
    gh api -X PATCH "repos/${GITHUB_REPOSITORY}/issues/comments/${existing}" -F "body=@${body}" \
        >/dev/null || echo "could not update comment (fork PR?)"
else
    gh pr comment "$PR_NUMBER" --body-file "$body" >/dev/null ||
        echo "could not post comment (fork PR?)"
fi
