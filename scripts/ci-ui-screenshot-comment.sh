#!/bin/sh
# PR に「この変更で画面がどう変わるか」を 1 個のコメントとして出す (CI 専用)。
#
# スナップショットは画像 (docs/preview/*.svg) なので、差分そのものを本文に貼ることはできない。
# 代わりに **before | diff | after を 1 行に並べて** 見せる。真ん中の diff は shotdiff
# (https://github.com/Matuyuhi/shotdiff) の `--diff-only` で、変わった画素だけをピンクに
# 塗った 1 枚。全画面を目で見比べなくても「どこが変わったか」だけが浮かぶ。
#
# **after と diff はこの実行で描き直した実物**を使う (コミット済みの画像ではない)。
# 作者が `--update-snapshots` を忘れていてもレビュアーには現在の描画が見える、というのが
# 狙い。ただしどちらもリポジトリに無いファイルなので、置き場として履歴を持たない
# orphan ブランチ (BRANCH) へ push し、その **コミット SHA** を URL に使う
# (ブランチ名で参照すると GitHub の画像プロキシが古い絵をキャッシュし続ける)。
# 生成物しか置かないブランチなので毎回 1 コミットに潰して force push する
# (放っておくと PNG が履歴に積み上がってリポジトリが太る)。
#
# コメントは毎回追加せず、隠しマーカーで自分の前回コメントを探して編集する
# (PR を更新するたびに同じ内容が積み上がるのを防ぐ)。gh は runner に同梱されている
# ものを使い、Action は増やさない (このリポジトリの CI は checkout / cache のみ)。
set -eu

MARKER='<!-- fv:ui-screenshot-diff -->'
DIR=docs/preview
# 生成物 (diff の PNG と描き直した SVG) の置き場。中身は全て再生成できるので、
# 邪魔になったらブランチごと消してよい
BRANCH=ci-ui-diff
# 3 列並ぶので 1 枚は小さめ。細部は画像をクリックして原寸で見る
WIDTH=300

# 生成物を orphan ブランチへ push して、そのコミット SHA を stdout に返す。
# 失敗 (fork PR の読み取り専用トークン等) は戻り値で伝え、呼び出し側が画像なしへ落とす
push_artifacts() {
    work=$1
    url="https://x-access-token:${GH_TOKEN}@github.com/${GITHUB_REPOSITORY}"
    repo="$work/branch"
    # 既にあれば他の PR のぶんを残したまま自分のディレクトリだけ差し替える
    git clone --quiet --depth 1 --branch "$BRANCH" "$url" "$repo" 2>/dev/null || {
        git init --quiet "$repo"
        git -C "$repo" remote add origin "$url"
    }
    rm -rf "$repo/pr-${PR_NUMBER}"
    cp -r "$work/pr-${PR_NUMBER}" "$repo/"
    git -C "$repo" config user.name "github-actions[bot]"
    git -C "$repo" config user.email "41898282+github-actions[bot]@users.noreply.github.com"
    # 履歴を持たせない: 毎回 1 コミットに潰して force push する
    git -C "$repo" checkout --quiet --orphan squashed
    git -C "$repo" add -A
    git -C "$repo" commit --quiet -m "ci: PR #${PR_NUMBER} の UI 差分"
    git -C "$repo" push --quiet --force origin "squashed:refs/heads/$BRANCH"
    git -C "$repo" rev-parse HEAD
}

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

# base と**今の作業ツリー** (直前のステップが描き直した実物) の差 = この PR による UI の変化。
# 作者がコミット済みかどうかとは独立に出す。
# $DIR には README.md 等シーンではないファイルも置けるので、拡張子で絞ってから
# シーン名に変換する (絞らないと README.md を「シーン README」として扱ってしまい、
# 存在しない README.md.svg を探しに行って落ちる)
changed=$(git diff --name-only FETCH_HEAD -- "$DIR" | grep '\.svg$' |
    sed "s|^$DIR/||;s|\.svg$||" | sort)
# 作業ツリーと HEAD のずれ = 更新し忘れているシーン
stale=$(git status --porcelain -- "$DIR" | awk '{print $NF}' | grep '\.svg$' |
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
    work=$(mktemp -d)
    out="$work/pr-${PR_NUMBER}"
    mkdir -p "$out"
    for scene in $changed; do
        after="$DIR/$scene.svg"
        cp "$after" "$out/$scene.after.svg"
        # base に無い (新しく足した) シーンは比較相手がいないので diff を作らない
        git cat-file -e "FETCH_HEAD:$after" 2>/dev/null || continue
        git show "FETCH_HEAD:$after" >"$work/before.svg"
        # 失敗しても (サイズ不一致等) diff の列が欠けるだけにする
        shotdiff "$work/before.svg" "$after" --diff-only -o "$out/$scene.diff.png" ||
            echo "::warning::shotdiff failed for $scene"
    done

    hosted=0
    art_sha=""
    if [ -n "$changed" ] && art_sha=$(push_artifacts "$work" 2>/dev/null); then
        hosted=1
    else
        echo "could not publish artifacts (fork PR?); falling back to committed images"
    fi

    printf '### UI スクリーンショット差分\n\n' >>"$body"
    if [ -n "$stale" ]; then
        printf '> [!NOTE]\n' >>"$body"
        printf '> コミット済みの画像が現在の描画と食い違っています (%s)。\n' \
            "$(echo "$stale" | tr '\n' ' ' | sed 's/ $//')" >>"$body"
        printf '> `cargo preview --update-snapshots` で更新してコミットしてください\n' >>"$body"
        printf '> (忘れたまま merge しても、main 側で自動的に追従コミットが積まれます)。\n\n' >>"$body"
    fi
    printf '変わったシーン: **%s**\n\n' \
        "$(echo "$changed" | tr '\n' ',' | sed 's/,$//;s/,/, /g')" >>"$body"

    # 1 シーンだけなら開いた状態で出す (クリックさせる意味がないため)
    count=$(echo "$changed" | grep -c . || true)
    [ "$count" -eq 1 ] && open=" open" || open=""
    for scene in $changed; do
        file="$DIR/$scene.svg"
        if [ "$hosted" = 1 ]; then
            art="https://raw.githubusercontent.com/${GITHUB_REPOSITORY}/${art_sha}/pr-${PR_NUMBER}"
            after_img="$art/$scene.after.svg"
            diff_img="$art/$scene.diff.png"
        else
            after_img="https://raw.githubusercontent.com/${HEAD_REPO}/${HEAD_SHA}/${file}"
            diff_img=""
        fi
        if git cat-file -e "FETCH_HEAD:$file" 2>/dev/null; then
            before_cell="<img width=\"$WIDTH\" src=\"https://raw.githubusercontent.com/${GITHUB_REPOSITORY}/${base_sha}/${file}\">"
        else
            before_cell="_(新規)_"
            diff_img=""
        fi
        if [ -n "$diff_img" ] && [ -f "$out/$scene.diff.png" ]; then
            diff_cell="<img width=\"$WIDTH\" src=\"$diff_img\">"
        else
            diff_cell="—"
        fi
        {
            printf '<details%s><summary><code>%s</code></summary>\n\n' "$open" "$scene"
            printf '| before | diff | after |\n| --- | --- | --- |\n'
            printf '| %s | %s | <img width="%s" src="%s"> |\n\n' \
                "$before_cell" "$diff_cell" "$WIDTH" "$after_img"
            printf '</details>\n\n'
        } >>"$body"
    done
    printf '_diff は [shotdiff](https://github.com/Matuyuhi/shotdiff) が変わった画素をピンクで塗ったもの。' >>"$body"
    printf 'diff と after は**この実行で描き直した実物**です。_\n' >>"$body"
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
