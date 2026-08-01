#!/bin/sh
# PR に「この変更で UI がどう変わるか」を 1 個のコメントとして出す (CI 専用)。
#
# 比較対象は base ブランチと**今の作業ツリー**。ワークフローの直前のステップが
# スナップショットを描き直しているので、作者が更新を忘れていても「現在の UI」との
# 差分が出る (だから always() で呼ばれる)。
#
# 1 シーンぶんの画面は 35 行あり、そのまま貼ると会話が流れてしまう。シーンごとに
# <details> で畳み、見出しに名前と増減行数だけ出して必要なものだけ開かせる。
#
# コメントは毎回追加せず、隠しマーカーで自分の前回コメントを探して編集する
# (PR を更新するたびに同じ内容が積み上がるのを防ぐ)。gh は runner に同梱されている
# ものを使い、Action は増やさない (このリポジトリの CI は checkout / cache のみ)。
set -eu

MARKER='<!-- fv:ui-snapshot-diff -->'
# GitHub のコメント上限は 65536 文字。余裕を持って切り、続きは Files changed で読ませる
MAX_BYTES=55000

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
    sections=$(mktemp)
    # 1 シーンだけなら開いた状態で出す (クリックさせる意味がないため)
    changed=$(printf '%s\n' "$diff" | grep -c '^diff --git ' || true)
    [ "$changed" -eq 1 ] && open=" open" || open=""

    summary=$(printf '%s\n' "$diff" | LC_ALL=C awk \
        -v out="$sections" -v max_bytes="$MAX_BYTES" -v open="$open" '
        function flush() {
            if (file == "") return
            if (done || bytes + length(buf) > max_bytes) {
                done = 1; skipped++
            } else {
                bytes += length(buf)
                printf "<details%s><summary><code>%s</code>%s (+%d -%d)</summary>\n\n```diff\n%s```\n\n</details>\n\n",
                    open, file, tag, plus, minus, buf > out
                names = names (names == "" ? "" : ", ") file
            }
            file = ""; buf = ""; plus = 0; minus = 0; tag = ""
        }
        /^diff --git / {
            flush()
            file = $NF
            sub(/^b\//, "", file); sub(/^tests\/snapshots\//, "", file); sub(/\.txt$/, "", file)
            next
        }
        # ヘッダ類は見出しで代用できるので落とす (画面そのものだけを残して読みやすくする)
        /^new file mode/ { tag = " (新規)"; next }
        /^deleted file mode/ { tag = " (削除)"; next }
        /^(index |--- |\+\+\+ |similarity |rename |old mode|new mode)/ { next }
        file != "" {
            if (substr($0, 1, 1) == "+") plus++
            else if (substr($0, 1, 1) == "-") minus++
            buf = buf $0 "\n"
        }
        END {
            flush()
            print names "\t" skipped + 0
        }
    ')
    names=${summary%%	*}
    skipped=${summary##*	}

    printf '### UI スナップショット差分\n\n' >>"$body"
    printf '変わったシーン: **%s**\n\n' "$names" >>"$body"
    cat "$sections" >>"$body"
    if [ "$skipped" -gt 0 ]; then
        printf '_他 %s シーンは長さの都合で省略しました。全体は Files changed タブで読めます。_\n' \
            "$skipped" >>"$body"
    fi
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
