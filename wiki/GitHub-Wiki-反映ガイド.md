# GitHub Wiki 反映ガイド

## 正本と更新順

公開Wikiは`https://github.com/salty919/codex_info_v2.wiki.git`という別Git repositoryの`master` branchです。本体repositoryの`wiki/*.md`はreview用mirrorであり、本体へcommitしただけでは公開ページへ反映されません。

更新順は次のとおりです。

1. 対応Issueと許可済みbranch / worktreeで本体側のMarkdownを更新する
2. 正本文書との整合、link、差分を確認する
3. 確定した5ページだけをWiki repositoryへcopyする
4. Wiki差分を再確認し、non-force pushする
5. remote commitと公開ページをread-backする

Codexが作業する場合は、先に本体repositoryの[AGENTS.md](https://github.com/salty919/codex_info_v2/blob/main/AGENTS.md)と[Issue #104の常駐必須条項](https://github.com/salty919/codex_info_v2/issues/104)を適用します。

## 同期するページ

- `Home.md`
- `導入と起動ガイド.md`
- `画面情報.md`
- `開発・運用メモ.md`
- `GitHub-Wiki-反映ガイド.md`

画像を変更する明示scopeがない場合、`wiki/*.png`や公開Wikiの画像はcopy・削除しません。

## 事前確認

`MAIN_WORKTREE`には、更新済みMarkdownを持つ許可済みworktreeの絶対pathを指定します。`WIKI_CHECKOUT`は既存pathを再利用せず、新しい空pathを指定します。

```bash
set -euo pipefail

MAIN_WORKTREE='/absolute/path/to/codex_info_v2-task-worktree'
WIKI_CHECKOUT='/absolute/path/to/new/codex_info_v2-wiki-checkout'

gh auth status
gh api repos/salty919/codex_info_v2 --jq '.has_wiki'
git -C "$MAIN_WORKTREE" status --short --branch
wiki_base="$(git ls-remote https://github.com/salty919/codex_info_v2.wiki.git refs/heads/master | awk '{print $1}')"
[[ "$wiki_base" =~ ^[0-9a-f]{40}$ ]]
test ! -e "$WIKI_CHECKOUT" && test ! -L "$WIKI_CHECKOUT"
git clone https://github.com/salty919/codex_info_v2.wiki.git "$WIKI_CHECKOUT"
test "$(git -C "$WIKI_CHECKOUT" branch --show-current)" = 'master'
test "$(git -C "$WIKI_CHECKOUT" rev-parse HEAD)" = "$wiki_base"
```

`ls-remote`は単一の40桁SHA、clone後のbranchは`master`でなければ停止します。認証情報をcommand、文書、commitへ埋め込みません。

## 5ページを同期する

```bash
set -euo pipefail

pages=(
  'Home.md'
  '導入と起動ガイド.md'
  '画面情報.md'
  '開発・運用メモ.md'
  'GitHub-Wiki-反映ガイド.md'
)

for page in "${pages[@]}"; do
  cp -- "$MAIN_WORKTREE/wiki/$page" "$WIKI_CHECKOUT/$page"
done

git -C "$WIKI_CHECKOUT" status --short
git -C "$WIKI_CHECKOUT" diff --check
git -C "$WIKI_CHECKOUT" diff -- "${pages[@]}"
```

差分に5ページ以外の変更、secret、private data、意図しない画像変更があればcommitしません。

## commitと公開

push直前にWikiのremote SHAを再取得し、clone時の`origin/master`から進んでいないことを確認します。進んでいた場合はforce pushせず停止し、remote変更を取り込んだ新しい差分をreviewします。

```bash
set -euo pipefail

expected="$(git -C "$WIKI_CHECKOUT" rev-parse origin/master)"
actual="$(git ls-remote https://github.com/salty919/codex_info_v2.wiki.git refs/heads/master | awk '{print $1}')"
test "$actual" = "$expected"

git -C "$WIKI_CHECKOUT" add -- "${pages[@]}"
git -C "$WIKI_CHECKOUT" diff --cached --check
git -C "$WIKI_CHECKOUT" commit -m 'docs: sync GitHub Wiki with current product guidance (Refs #<issue>)'
git -C "$WIKI_CHECKOUT" push origin HEAD:master
```

`<issue>`は実際のIssue番号へ置き換えます。GitHub WikiにはPull Request経路がないため、このpushで公開内容が直接更新されます。

## 公開後の確認

次をすべて確認します。

- `git ls-remote`の`master` SHAがpushしたcommitと一致する
- [Wiki Home](https://github.com/salty919/codex_info_v2/wiki)と4つの子ページが表示できる
- 本体worktreeの5 Markdownと、公開Wikiの同名5ファイルがbyte単位で一致する
- Wiki内linkと正本文書へのlinkが目的のページへ到達する

pushまたはread-backに失敗した場合は公開済みと報告せず、本体側のcommit、Wiki側のcommit、remote SHAを分けて記録します。同じ状態のままpushを繰り返したり、force pushで上書きしたりしません。

## cleanup

本体側はrepositoryのbranch / worktree規則に従います。Wiki checkoutは未commit・未push差分がないことを確認してから削除し、未公開のunique commitがある場合は保持して復旧方法を報告します。
