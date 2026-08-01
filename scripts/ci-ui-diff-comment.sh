#!/bin/sh
# PR に「この変更で UI がどう変わるか」を 1 個のコメントとして出す (CI 専用)。
#
# 比較対象は base ブランチと**今の作業ツリー**。ワークフローの直前のステップが
# スナップショットを描き直しているので、作者が更新を忘れていても「現在の UI」との
# 差分が出る (だから always() で呼ばれる)。
#
# コメントは毎回追加せず、隠しマーカーで自分の前回コメントを探して編集する
# (PR を更新するたびに同じ内容が積み上がるのを防ぐ)。gh は runner に同梱されている
# ものを使い、Action は増やさない (このリポジトリの CI は checkout / cache のみ)。
set -eu

MARKER='<!-- fv:ui-snapshot-diff -->'
# GitHub のコメント上限は 65536 文字。余裕を持って切り、続きは Files changed で読ませる。
# 行数でも切るのは、シーンを大量に足した PR で「読む気の起きない長さ」になるのを防ぐため
MAX_BYTES=55000
MAX_LINES=400

if [ "${GITHUB_EVENT_NAME:-}" != "pull_request" ]; then
    echo "not a pull request; skip"
    exit 0
fi

# shallow clone のままでも良いよう、base の先端だけ取って 2 点間で比較する
# (マージベースを必要としないので --depth=1 で足りる)
git fetch --no-tags --depth=1 origin "$GITHUB_BASE_REF" >/dev/null 2>&1 || {
    echo "could not fetch base ref; skip"
    exit 0
}

diff=$(git diff FETCH_HEAD -- tests/snapshots || true)
existing=$(gh api "repos/${GITHUB_REPOSITORY}/issues/${PR_NUMBER}/comments" --paginate \
    --jq "map(select(.body | startswith(\"${MARKER}\"))) | .[-1].id // empty" 2>/dev/null || true)

body=$(mktemp)
printf '%s\n' "$MARKER" >>"$body"

if [ -z "$diff" ]; then
    # 差分が無いのに新しくコメントを作ると、UI に無関係な PR にまで雑音が出る。
    # 既にコメントがある時だけ「差分は無くなった」と上書きする
    if [ -z "$existing" ]; then
        echo "no UI diff and no existing comment; skip"
        exit 0
    fi
    printf '### UI スナップショット\n\nこの PR による UI の差分はありません。\n' >>"$body"
else
    printf '### UI スナップショット差分\n\n' >>"$body"
    printf '`tests/snapshots/` を base と比較した結果です。意図しない変化が無いか確認してください。\n\n' >>"$body"
    printf '```diff\n' >>"$body"
    # 切るのは必ず行単位。バイト数で切ると罫線や日本語が途中で割れて化ける
    printf '%s\n' "$diff" | LC_ALL=C awk -v max_bytes="$MAX_BYTES" -v max_lines="$MAX_LINES" '
        { bytes += length($0) + 1 }
        bytes > max_bytes || NR > max_lines { truncated = 1; exit }
        { print }
        END { if (truncated) print "... (省略。全体は Files changed タブで読めます)" }
    ' >>"$body"
    printf '```\n' >>"$body"
fi

# fork からの PR では GITHUB_TOKEN が読み取り専用でコメントできない。
# それだけで CI を落とす理由は無いので、失敗しても警告に留める
if [ -n "$existing" ]; then
    gh api -X PATCH "repos/${GITHUB_REPOSITORY}/issues/comments/${existing}" -F "body=@${body}" \
        >/dev/null || echo "could not update comment (fork PR?)"
else
    gh pr comment "$PR_NUMBER" --body-file "$body" >/dev/null ||
        echo "could not post comment (fork PR?)"
fi
