#!/bin/sh
# PR に「この変更で画面がどう変わるか」を 1 個のコメントとして出す (CI 専用)。
#
# スナップショットは画像 (docs/preview/*.svg) なので、差分そのものを本文に貼ることはできない。
# 代わりに **base と head の画像を raw URL で並べて** 見せる — GitHub がコメント内で SVG を
# 描画するので、Files changed を開かなくても会話の流れの中で新旧を見比べられる。
#
# 「head の画像」はコミット済みのものを指す。作者が更新を忘れていると現在の描画と食い違うので、
# 直前のステップが描き直した結果と HEAD がずれている (= stale) 場合はその旨を先頭に出す
# (main への push では自動追従するので、PR で更新するかどうかは作者に委ねる)。
#
# コメントは毎回追加せず、隠しマーカーで自分の前回コメントを探して編集する
# (PR を更新するたびに同じ内容が積み上がるのを防ぐ)。gh は runner に同梱されている
# ものを使い、Action は増やさない (このリポジトリの CI は checkout / cache のみ)。
set -eu

MARKER='<!-- fv:ui-screenshot-diff -->'
DIR=docs/preview
# 1 シーンあたり新旧 2 枚。狭くしすぎると画面として読めないので、横 2 列で収まる幅にする
WIDTH=460

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
base_sha=$(git rev-parse FETCH_HEAD)

# この PR がコミットした画像の変化 (= レビュアーに見せたい新旧)
changed=$(git diff --name-only FETCH_HEAD -- "$DIR" | sed "s|^$DIR/||;s|\.svg$||" | sort)
# 直前のステップが描き直した結果と HEAD のずれ (= 更新し忘れているシーン)
stale=$(git status --porcelain -- "$DIR" | awk '{print $NF}' |
    sed "s|^$DIR/||;s|\.svg$||" | sort)

existing=$(gh api "repos/${GITHUB_REPOSITORY}/issues/${PR_NUMBER}/comments" --paginate \
    --jq "map(select(.body | startswith(\"${MARKER}\"))) | .[-1].id // empty" 2>/dev/null || true)

body=$(mktemp)
printf '%s\n' "$MARKER" >>"$body"

if [ -z "$changed" ] && [ -z "$stale" ]; then
    # 差分が無いのに新しくコメントを作ると、UI に無関係な PR にまで雑音が出る。
    # 既にコメントがある時だけ「差分は無くなった」と上書きする
    if [ -z "$existing" ]; then
        echo "no UI diff and no existing comment; skip"
        exit 0
    fi
    printf '### UI スクリーンショット\n\nこの PR による UI の差分はありません。\n' >>"$body"
else
    printf '### UI スクリーンショット差分\n\n' >>"$body"
    if [ -n "$stale" ]; then
        printf '> [!NOTE]\n' >>"$body"
        printf '> コミット済みの画像が現在の描画と食い違っています (%s)。\n' \
            "$(echo "$stale" | tr '\n' ' ' | sed 's/ $//')" >>"$body"
        printf '> `cargo preview --update-snapshots` で更新してコミットすると下の "after" が実物になります\n' >>"$body"
        printf '> (更新しないまま merge しても、main 側で自動的に追従コミットが積まれます)。\n\n' >>"$body"
    fi
    if [ -n "$changed" ]; then
        printf '変わったシーン: **%s**\n\n' \
            "$(echo "$changed" | tr '\n' ',' | sed 's/,$//;s/,/, /g')" >>"$body"
    fi
    # 1 シーンだけなら開いた状態で出す (クリックさせる意味がないため)
    count=$(echo "$changed" | grep -c . || true)
    [ "$count" -eq 1 ] && open=" open" || open=""
    for scene in $changed; do
        file="$DIR/$scene.svg"
        before="https://raw.githubusercontent.com/${GITHUB_REPOSITORY}/${base_sha}/${file}"
        after="https://raw.githubusercontent.com/${HEAD_REPO}/${HEAD_SHA}/${file}"
        # base に無い (新しく足した) シーンは before を持たない
        if git cat-file -e "FETCH_HEAD:$file" >/dev/null 2>&1; then
            cell="<img width=\"$WIDTH\" src=\"$before\">"
        else
            cell="_(新規)_"
        fi
        {
            printf '<details%s><summary><code>%s</code></summary>\n\n' "$open" "$scene"
            printf '| before | after |\n| --- | --- |\n'
            printf '| %s | <img width="%s" src="%s"> |\n\n' "$cell" "$WIDTH" "$after"
            printf '</details>\n\n'
        } >>"$body"
    done
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
