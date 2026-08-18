#!/usr/bin/env python3
"""issue #94 の書き直しの受け入れ条件を測る。

使い方: python3 check-rewrite.py <リポジトリのルート> <定義ファイルの repo 相対パス> [退避先の repo 相対パス]

終了コード: 0 = 全合格 / 1 = 不合格 / 2 = 測定できていない（母数 0 など）

どの検査も母数を先に出力する。母数 0 は合格ではなく FATAL として扱う。
受け入れ条件は定義ファイル本体にだけ掛ける。退避先は射程外
（前例: kamux-promotion/references/case-studies.md が日付を保持したまま PR #93 で着地した）。

検査を通すために検査自身を緩めないこと。`--no-fence` を面倒だからという理由で付けた瞬間、
code fence の逐語比較は永久に空振りする。`--chapters=N` の N を実体に合わせて書き換えるだけなら、
逆向きの検査は「増えたことに気づく」機能を失う。どちらも、外す前に外してよい理由を PR 本文へ書くこと。

このスクリプトが測らないもの（限界。欠陥ではない）:

- **規範項目の欠落を測らない。** 見出し・code fence・手書きのアンカーしか見ないので、
  見出しと見出しの間の散文が 1 本消えても全検査が緑になる。frontmatter も母数の外。
  「全合格 / exit=0」を「欠落 0 件」の根拠に使ってはならない。欠落の唯一の根拠は
  台帳（co-inventory.md）の人手突き合わせである。
- **規則の本文が主張していることは読まない。** 例えば「章の書き方」節の本文を
  「どこでも § でよい」へ書き換えても検査 5 は緑のままになる。散文の主張は機械で測れない。
"""
import hashlib
from collections import Counter
import os
import re
import subprocess
import sys
import unicodedata

ROOT = os.path.abspath(sys.argv[1])
REL = sys.argv[2]
argv = [x for x in sys.argv[3:] if not x.startswith("--")]
FLAGS = {x for x in sys.argv[3:] if x.startswith("--")}
CASES_REL = argv[0] if argv else None

KNOWN = ("--no-fence", "--no-chapters", "--chapters=", "--renamed=", "--no-renamed", "--anchor=", "--no-anchors", "--no-decorated", "--no-cases", "--fence=", "--no-fence-change")
_bad = [f for f in FLAGS if not any(f == k or f.startswith(k) for k in KNOWN)]
if _bad:
    # typo を黙って受理すると、外したつもりの検査が走り、付けたつもりの逃げ道が効かない。
    print(f"FATAL [引数] 未知の引数 {_bad}。既知は {KNOWN}")
    sys.exit(2)
TARGET = os.path.join(ROOT, REL)
LEDGER = os.path.join(ROOT, ".claude/AGENT-LESSONS.md")

ng, fatal = [], []


def report(tag, ok, msg):
    print(f"{'OK ' if ok else 'NG '} [{tag}] {msg}")
    if not ok:
        ng.append(tag)


def die(tag, msg):
    print(f"FATAL [{tag}] {msg}")
    fatal.append(tag)


def read(path):
    if not os.path.exists(path):
        die("前提", f"{path} が無い")
        return None
    with open(path, encoding="utf-8") as f:
        return f.read()


def norm(s):
    """記号と空白を正規化してから比較する。装飾で切れた引用を拾うため。"""
    s = unicodedata.normalize("NFKC", s)
    return re.sub(r"[\s*`「」『』【】]", "", s)


def core(heading):
    """見出しから装飾を剥ぐ。🔴 と、末尾の（群 X。日付 …）を落とす。"""
    h = heading.replace("🔴", "").strip()
    h = re.sub(r"（[^（）]*(群 |20[0-9][0-9]-|書き直し|昇格|追加)[^（）]*）\s*$", "", h).strip()
    return h


def headings(text):
    return [core(l.lstrip("#").strip()) for l in text.split("\n") if re.match(r"^#{2,3} ", l)]


body = read(TARGET)
ledger = read(LEDGER)
if body is None or ledger is None:
    sys.exit(2)

old = subprocess.run(["git", "-C", ROOT, "show", f"origin/main:{REL}"], capture_output=True, text=True)
if old.returncode != 0:
    die("前提", f"旧版を取得できない: {old.stderr.strip()}")
    sys.exit(2)
old_body = old.stdout

lines = body.rstrip("\n").split("\n")
old_lines = old_body.rstrip("\n").split("\n")
print(f"--- 対象 {REL}（旧 {len(old_lines)} 行 → 新 {len(lines)} 行）")
if CASES_REL:
    c = read(os.path.join(ROOT, CASES_REL))
    print(f"--- 退避先 {CASES_REL}（{len(c.rstrip(chr(10)).split(chr(10))) if c else 0} 行。受け入れ条件の射程外）")
print()

# 1. 🔴 が 3 個以下
n = body.count("🔴")
report("1", n <= 3, f"🔴 = {n} 個（旧 {old_body.count('🔴')} 個 / 許容 3 個以下）")

# 2. 日付が 0 件。計画ディレクトリ名 plans/2026-08-01-kamux はパスの一部なので射程外。
#    除外が空振りしていないことを見るため、除外前後の件数を両方出す。
DATE = r"20[0-9][0-9]-[0-9][0-9]-[0-9][0-9]"
raw = re.findall(DATE, body)
hits = re.findall(DATE, re.sub(r"plans/" + DATE + r"-kamux", "plans/<計画ディレクトリ>", body))
report("2", not hits, f"日付 = {len(hits)} 件（旧 {len(re.findall(DATE, old_body))} 件 / パス由来 {len(raw) - len(hits)} 件を除外。除外前 {len(raw)} 件）{hits[:5]}")

# 3. 台帳の行番号が 0 件
hits = re.findall(r"台帳 *#", body)
report("3", not hits, f"台帳の行番号 = {len(hits)} 件（旧 {len(re.findall(r'台帳 *#', old_body))} 件）")

# 4. 200 文字を超える行が 0（バイトではなく文字で測る）
longs = [(i + 1, len(l)) for i, l in enumerate(lines) if len(l) > 200]
report("4", not longs, f"200 文字超 = {len(longs)} 行（最長 {max(len(l) for l in lines)} 文字 / 母数 {len(lines)} 行）{longs[:5]}")

# 5. § は識別子として書かれた所（code span / code fence）と「章の書き方」節にだけ残す。
#    地の文の § は NG。所在は全件出力して目でも見えるようにする。
def section_of(idx):
    """その行が属する ## 見出しを返す。"""
    cur = ""
    for i, l in enumerate(lines):
        if l.startswith("## "):
            cur = core(l[3:].strip())
        if i == idx:
            return cur
    return cur

def in_fence(idx):
    return sum(1 for l in lines[:idx] if l.startswith("```")) % 2 == 1

sec = [(i + 1, l.strip()[:90]) for i, l in enumerate(lines) if "§" in l]
bad5 = []
for i, l in enumerate(lines):
    if "§" not in l:
        continue
    if in_fence(i) or section_of(i) == "章の書き方":
        continue
    # 地の文の行は、§ が code span の中にあるものだけ許す
    if "§" in re.sub(r"`[^`]*`", "", l):
        bad5.append((i + 1, l.strip()[:80]))
# § が 1 個でも在るなら、どちらをいつ使うかを定める節が実在しなければならない。
#    節を消しても § は識別子として通ってしまうので、節の実在を別に測る。
has_sec = any(core(l[3:].strip()) == "章の書き方" for l in lines if l.startswith("## "))
report("5", not bad5 and (has_sec or "§" not in body),
       f"§ = {sum(l.count(chr(0xA7)) for l in lines)} 個 / {len(sec)} 行（旧 {old_body.count(chr(0xA7))} 個）。"
       f"識別子でも「章の書き方」節でもない § = {len(bad5)} 件 {bad5[:3]} / 「章の書き方」節 = {'在り' if has_sec else '無し'}")
for i, l in sec:
    print(f"        :{i} {l}")

# 5b. 逆向き。壊れたのは「素の § を書く」向きではなく、「契約へ書き込む値を N 章 で書く」向き。
#     どちらに当たるかは用途で決まるので機械では裁けない。母数を宣言させることで、
#     新しい N 章 が増えたことに書き手が気づく形にする。
# 切り捨てるのは表示だけ。ダイジェストの材料に切り捨て済みの値を流用すると、
# 切った位置より後ろの違反が素通りする。
chap = [(i + 1, l.strip()) for i, l in enumerate(lines) if re.search(r"[0-9]+ 章", l)]
declared = next((f for f in FLAGS if f.startswith("--chapters=")), None)
if not chap:
    # I-3 で閉じたのと同じ形をここで開けない。0 行が正しいなら --no-chapters を明示する。
    if "--no-chapters" in FLAGS:
        print("SKIP [5b] 「N 章」を含む行が 0 行（--no-chapters で明示的に外した）")
    else:
        die("5b", "「N 章」を含む行が 0 行。母数 0 は合格ではない。無いことが正しいなら --no-chapters を渡す")
elif "--no-chapters" in FLAGS:
    die("5b", f"--no-chapters を渡したが、「N 章」を含む行が {len(chap)} 行ある。外す理由が成立していない")
elif declared is None:
    die("5b", f"「N 章」を含む行が {len(chap)} 行ある。用途は機械で裁けないので、"
              f"人が全件を裁いたうえで --chapters={len(chap)} を渡すこと")
else:
    # 件数だけを宣言させると「1 箇所直して 1 箇所足す」入れ替えが素通りする。
    # 該当行の中身のダイジェストまで宣言させる。行番号は入れないので、
    # 無関係な編集で行がずれただけでは変わらない。
    digest = hashlib.sha256(
        "\n".join(sorted(l for _, l in chap)).encode("utf-8")
    ).hexdigest()[:12]
    actual = f"{len(chap)}:{digest}"
    want = declared.split("=", 1)[1]
    if ":" not in want:
        # 正しい呼び出し方を散文に置くと、器が変わるたびに古くなる。器の側から出す。
        die("5b", f"--chapters は <件数>:<ダイジェスト> の形で渡す。下の全件を人が裁いてから、"
                  f"次のコマンドを打つこと:\n"
                  f"       python3 .claude/scripts/check-rewrite.py . {REL} "
                  f"{CASES_REL or ''} --chapters={actual}")
    else:
        ok5b = want == actual
        report("5b", ok5b,
               f"逆向き（契約へ書き込む値を「N 章」で書いていないか）: 実体 {actual} / 宣言 {want}。"
               f"全件を出すので用途を人が裁くこと")
        if not ok5b:
            # 宣言値は人が手で書き写す。器の変更でダイジェストの材料が変わると、
            # 過去に承認された宣言が黙って古くなる。正解を器の側から出す。
            print(f"        下の全件を人が裁き直してから: --chapters={actual}")
    for i, l in chap:
        print(f"        :{i} {l[:90]}")

# 6. 太字が見出しと表の項目名まで
n = body.count("**") // 2
report("6", n <= 8, f"太字 = {n} 箇所（旧 {old_body.count('**') // 2} 箇所 / 許容 8 箇所以下）")

# 7. 旧見出しの全文（装飾込み）への参照が 0 件
old_full = [l.lstrip("#").strip() for l in old_lines if re.match(r"^#{2,3} ", l)]
decorated = [h for h in old_full if h != core(h)]
if not decorated:
    # origin/main 側が既に書き直し済みのファイル（1 本目の再検算など）では 0 本が正しい。
    if "--no-decorated" in FLAGS:
        print("SKIP [7] 装飾付きの旧見出しが 0 本（--no-decorated で明示的に外した）")
    else:
        die("7", "装飾付きの旧見出しが 0 本。母数 0 は合格ではない。"
                 "旧版が既に書き直し済みなら --no-decorated を渡す")
elif "--no-decorated" in FLAGS:
    die("7", f"--no-decorated を渡したが、装飾付きの旧見出しが {len(decorated)} 本ある。外す理由が成立していない")
else:
    stale, own = [], 0
    for h in decorated:
        r = subprocess.run(["grep", "-rnF", "--include=*.md", "--exclude-dir=worktrees", h, ".claude/", "CLAUDE.md"],
                           capture_output=True, text=True, cwd=ROOT)
        # grep は「見つからない」で 1 を返す。2 以上はエラーで、失敗が「参照 0 件」と同じ出力になる。
        if r.returncode > 1:
            die("7", f"参照の検索に失敗した（exit {r.returncode}）: {r.stderr.strip()}")
        for line in r.stdout.strip().split("\n"):
            if not line or line.startswith(REL):
                continue
            # 他の定義ファイルが「自分の見出し」として同じ文字列を持つ行は参照ではない。
            # 「行頭が # か」だけで寄せると、古いポインタを見出しの形で書けば不可視になる。
            # 除外は 3 条件すべてを満たすときだけ —— (1) 定義ファイルである
            # (2) 見出しである (3) 見出しの本文が旧見出しと逐語一致する。
            # 退避先や台帳は定義ファイルではないので、そこに置いた古いポインタは除外しない。
            path, text = line.split(":", 1)[0], line.split(":", 2)[-1]
            is_def = re.fullmatch(r"\.claude/agents/[\w.-]+\.md|\.claude/skills/.+/SKILL\.md", path)
            if is_def and re.match(r"^#{1,6} ", text.lstrip()) and text.lstrip().lstrip("#").strip() == h:
                own += 1
                continue
            stale.append(line[:120])
    report("7", not stale, f"旧見出し（装飾込み）への参照 = {len(stale)} 件 / 母数 {len(decorated)} 本"
                           f"（他ファイル自身の見出し {own} 件は除外）{stale[:3]}")

# 8. 向き 1: 旧見出しのコアが、新版の見出しとして 1 対 1 で実在するか。
#    装飾を剥ぐだけのはずなので、コアが消えていたらそれは改名である。
new_heads = headings(body)
if not old_full or not new_heads:
    die("8", f"母数が壊れている（旧見出し {len(old_full)} 本 / 新見出し {len(new_heads)} 本）")
else:
    new_n = [norm(h) for h in new_heads]
    lost = sorted(core(h) for h in old_full if norm(core(h)) not in new_n)
    if not lost:
        # 改名 0 本なのに古い --renamed= が残っていると、次のファイルでも通ってしまう。
        # 5b と同じく両方向を閉じる（片側だけ閉じるのが台帳 #243 の形）。
        if any(f.startswith("--renamed=") for f in FLAGS):
            die("8", "改名された旧見出しは 0 本なのに --renamed= が渡されている。外す理由が成立していない")
        else:
            report("8", True, f"向き 1: 旧見出しのコア {len(old_full)} 本のうち新版に無いもの 0 本")
    else:
        # 意図的な改名はありうる（見出しに章参照が埋まっている等）。件数だけでは
        # 「1 本直して 1 本消す」が素通りするので、5b と同じくダイジェストまで宣言させる。
        rd = f"{len(lost)}:{hashlib.sha256(chr(10).join(lost).encode('utf-8')).hexdigest()[:12]}"
        decl = next((f.split("=", 1)[1] for f in FLAGS if f.startswith("--renamed=")), None)
        if "--no-renamed" in FLAGS:
            die("8", f"--no-renamed を渡したが、新版に無い旧見出しのコアが {len(lost)} 本ある: {lost}")
        elif decl is None:
            die("8", f"新版に無い旧見出しのコアが {len(lost)} 本ある。参照切れが無いことは検査 9 / 9b が見るが、"
                     f"改名そのものが意図的かは人が裁く。下を読んでから次を渡すこと:\n"
                     f"       --renamed={rd}\n" + "\n".join(f"       - {x}" for x in lost))
        else:
            report("8", decl == rd, f"向き 1: 旧見出しのコア {len(old_full)} 本のうち新版に無いもの {len(lost)} 本。"
                                    f"実体 {rd} / 宣言 {decl}。全件: {lost}")

# 9. 向き 2: 「旧版で解決していた引用」が新版でも解決するか。
#    .claude/ 配下の全 .md から 「…」 を拾い、旧見出しに当たるものだけを母数にする。
#    ファイル名と引用の距離に依存しないので、書き方の揺れで母数を落とさない。
# 母数は origin/main から取る。作業ツリーから取ると、参照元を同じ便で直したときに
# 母数が黙って縮み、切れた参照が母数の外へ逃げる。
quoted = set()
r = subprocess.run(["git", "-C", ROOT, "grep", "-h", "-e", "「", "origin/main", "--", ".claude/*.md", ".claude/**/*.md"],
                   capture_output=True, text=True)
if r.returncode not in (0, 1):
    die("9", f"origin/main から引用を取得できない: {r.stderr.strip()}")
for line in r.stdout.split("\n"):
    for q in re.findall(r"「([^」]+)」", line):
        quoted.add(q.strip())
old_cores = [norm(core(h)) for h in old_full]


def strip_ellipsis(q):
    """他ファイルは見出しを「純関数へ切り出したら…」の形で省略して指す。
    末尾の … を落とさないと完全一致にも部分一致にも当たらず、母数から漏れる。"""
    return re.sub(r"(…|\.\.\.)\s*$", "", q).strip()


def hits_old(q):
    """旧見出しを指していた引用か。部分一致だけで拾うと、「該当 0 件」のような
    一般的な断片が見出しの中に在るせいで母数に入り、誰も指していない見出しを
    改名しただけで NG が出る（偽陽性）。判定に使うのは完全一致と、
    末尾を省略した前方一致だけ。"""
    qn = norm(q)
    if qn in old_cores:
        return True
    e = norm(strip_ellipsis(q))
    return e != qn and len(e) >= 4 and any(c.startswith(e) for c in old_cores)


def loosely(q):
    qn = norm(q)
    return len(qn) >= 4 and any(qn in c or c in qn for c in old_cores)
# 厳格な部分母数: 対象ファイル名と同じ行に書かれた引用だけを取る。こちらが本物のポインタ。
base = os.path.basename(REL)
r2 = subprocess.run(["git", "-C", ROOT, "grep", "-h", "-e", base, "origin/main", "--", ".claude/*.md", ".claude/**/*.md"],
                    capture_output=True, text=True)
if r2.returncode > 1:
    die("9b", f"origin/main から {base} を含む行を取得できない（exit {r2.returncode}）: {r2.stderr.strip()}")
named = sorted({q for line in r2.stdout.split("\n") for q in re.findall(r"「([^」]+)」", line) if loosely(q)})
# 判定の母数 = 旧見出しと完全一致する引用 ∪ 対象ファイル名と同じ行に書かれた引用。
# 前者は曖昧さが無く、後者は文脈でポインタだと分かる。どちらでもない部分一致は
# 一覧には出すが判定には使わない。
resolved_before = sorted({q for q in quoted if hits_old(q)} | set(named))
advisory = sorted({q for q in quoted if loosely(q) and not hits_old(q) and q not in named})
if not resolved_before:
    die("9", "旧版で解決していた引用が 0 件。母数が壊れている")
else:
    new_n = [norm(h) for h in headings(body)]
    body_n = norm(body)
    # 母数は「旧版で見出しに解決していた引用」なので、新版でも見出しに解決しなければならない。
    # 本文のどこかに在るだけでは合格にしない（節名を指すポインタが散文の一部として
    # 残っただけの状態を通してしまう）。
    def resolves(q):
        qn, e = norm(q), norm(strip_ellipsis(q))
        return any(qn in h or h in qn or h.startswith(e) for h in new_n)

    broken = [q for q in resolved_before if not resolves(q)]
    broken_named = [q for q in named if q in broken]
    report("9", not broken, f"向き 2: 旧版で解決していた引用 {len(resolved_before)} 件のうち新版で切れたもの {len(broken)} 件 {broken}")
    if not named:
        # 母数 0 のまま OK を出さない。9 には die が在るのに 9b だけ無かった。
        die("9b", f"{base} と同じ行に書かれた引用が 0 件。母数 0 は合格ではない。"
                  f"他ファイルがこのファイルを節名で指していないか、grep が失敗している")
    else:
        report("9b", not broken_named, f"向き 2（厳格）: {base} と同じ行に書かれた引用 {len(named)} 件のうち切れたもの {len(broken_named)} 件 {broken_named}")
    print(f"        厳格母数の内訳: {named}")
    print(f"        判定に使わない部分一致 {len(advisory)} 件（見出しの断片にたまたま当たるだけ）: {advisory}")
    for q in resolved_before:
        # 判定は見出しへの解決だけを合格にするので、一覧の表記もそれに合わせる。
        where = ("見出し" if resolves(q)
                 else ("切れた（本文にはあるが見出しではない）" if norm(q) in body_n else "切れた"))
        print(f"        「{q}」→ {where}")

# 10. 退避先へのポインタが本文に 1 行あり、実体が在る
#     引数を落とすと検査ごと消える形を閉じる。5b / 7 / 11 / 12 には「外す理由が成立しない」
#     FATAL が在るのに 10 だけ無く、退避先を渡し忘れると 1 行も出力せず全合格に到達していた。
#     フラグは操作者の別の引数ではなく本文の実体と突き合わせる。--no-fence が旧版の fence を、
#     --no-decorated が旧見出しを見るのと同じ形にする。パスを渡さずフラグだけ渡す逃げ道を塞ぐ。
_ref = re.search(r"\.claude/references/[\w.-]*cases\.md", body)
if not CASES_REL:
    if "--no-cases" in FLAGS and _ref:
        die("10", f"--no-cases を渡したが、本文が退避先 {_ref.group(0)} を指している。外す理由が成立していない")
    elif "--no-cases" in FLAGS:
        print("SKIP [10] 退避先が無い（--no-cases で明示的に外した。本文にも退避先への参照が無い）")
    else:
        die("10", "退避先の引数が無い。母数 0 は合格ではない。退避先を作っていないなら --no-cases を渡す")
elif "--no-cases" in FLAGS:
    die("10", f"--no-cases を渡したが、退避先 {CASES_REL} が引数に在る。外す理由が成立していない")
else:
    ptr = CASES_REL in body
    report("10", ptr and os.path.exists(os.path.join(ROOT, CASES_REL)),
           f"退避先へのポインタ {'在り' if ptr else '無し'} / 実体 {'在り' if os.path.exists(os.path.join(ROOT, CASES_REL)) else '無し'}")

# 11. 旧版の code fence の中身が、実行行はバイト単位で保存されているか。
#     コメント行（# で始まる）は表記を直しうるので除く。
def code_lines(text):
    """fence の中身を全部返す。コメント行も含める。
    コメントは規範値を運んでいる（「契約 §7 のコマンド 21 個」「§8 の 4 トピック」）ので、
    除外すると § を保ったまま 21 → 20 や 4 → 3 に変えられても誰も気づかない。"""
    out, inside = [], False
    for l in text.split("\n"):
        if l.startswith("```"):
            inside = not inside
            continue
        if inside and l.strip():
            out.append(l)
    return out

def fence_seq(text):
    """fence の中身を空行も含めて並び順のまま返す。
    code_lines() は多重集合の材料なので空行を捨てるが、それだと
    「行を入れ替える」「空行を挟む」変異が差分 0 として素通りする。"""
    out, inside = [], False
    for l in text.split("\n"):
        if l.startswith("```"):
            inside = not inside
            continue
        if inside:
            out.append(l)
    return out


# 新版に `> ` 付きの fence が在ると、code_lines() が fence と数えないので
# 中身が逐語保護の外へ出る。旧版にはこの形が実在し、本検査の射程外だった。
_quoted = [i + 1 for i, l in enumerate(lines) if l.startswith("> ```")]
if _quoted:
    die("11", f"新版に `> ` 付きの code fence が {len(_quoted)} 本ある（行 {_quoted}）。"
              f"この形は逐語保護の外に出る。素の fence にすること")

old_code, new_code = code_lines(old_body), code_lines(body)
if not old_code:
    # 母数 0 を黙って通さない。旧版に fence が無いファイルでは --no-fence を明示的に渡す。
    if "--no-fence" in FLAGS:
        print("SKIP [11] 旧版に code fence が無い（--no-fence で明示的に外した）")
    else:
        die("11", "旧版に code fence が 0 行。母数 0 は合格ではない。無いことが正しいなら --no-fence を渡す")
elif "--no-fence" in FLAGS:
    # フラグと本文の矛盾を先に見る。前例は check-promotion-pr.sh の ng 4。
    die("11", f"--no-fence を渡したが、旧版に code fence が {len(old_code)} 行ある。外す理由が成立していない")
else:
    # 多重集合で比べる。集合で比べると同じ行を複製しても気づかない。
    co, cn = Counter(old_code), Counter(new_code)
    missing = list((co - cn).elements())
    added = list((cn - co).elements())
    head = (f"code fence の行（コメント込み）: 旧 {len(old_code)} 行 / 新 {len(new_code)} 行。"
            f"落ちた {len(missing)} 行 / 増えた {len(added)} 行")
    # 逐語保存が既定である。ただし fence の中に「規範値ではない見本」が混ざることがあり
    # （日付の見本など）、他の検査（2 の日付）と正面から衝突する。5b / 8 / 12 と同じ形で
    # 宣言の逃げ道を 1 つ置く。件数だけでは「1 行落として 1 行足す」入れ替えが素通りするので
    # ダイジェストまで宣言させる。--no-fence は「旧版に fence が無い」の意味なので兼用しない。
    # 増えた側は「新版に現れる順」で固定する。sorted() で並べると、
    # 増えた行どうしの入れ替えが多重集合を変えないまま素通りする
    # （旧版が `> ` 付きだった fence を素の fence へ移した便では、
    #  そのブロック全体が added なので 11b でも比べられない）。
    _cnt_added = Counter(added)
    added_ordered = []
    for l in new_code:
        if _cnt_added[l] > 0:
            _cnt_added[l] -= 1
            added_ordered.append(l)
    delta = sorted(missing) + added_ordered
    fdecl = next((f.split("=", 1)[1] for f in FLAGS if f.startswith("--fence=")), None)
    if not delta:
        if fdecl is not None:
            die("11", "fence の差分は 0 行なのに --fence= が渡されている。外す理由が成立していない")
        elif "--no-fence-change" in FLAGS:
            print(f"SKIP [11] {head}（--no-fence-change。差分 0 行なので宣言は不要だった）")
        else:
            report("11", True, head)
    else:
        actual = f"{len(delta)}:{hashlib.sha256(chr(10).join(delta).encode('utf-8')).hexdigest()[:12]}"
        if "--no-fence-change" in FLAGS:
            die("11", f"--no-fence-change を渡したが、fence の差分が {len(delta)} 行ある。外す理由が成立していない")
        elif fdecl is None:
            die("11", f"{head}。**逐語保存が既定である。** 意図した変更なら、下の全件を人が裁いたうえで次を渡すこと:\n"
                      f"       --fence={actual}\n"
                      + "\n".join(f"       - 落ちた: {x}" for x in sorted(missing))
                      + ("\n" if missing and added else "")
                      + "\n".join(f"       - 増えた: {x}" for x in sorted(added)))
        else:
            ok11 = fdecl == actual
            report("11", ok11, f"{head}。実体 {actual} / 宣言 {fdecl}")
            if not ok11:
                print(f"        下の全件を人が裁き直してから: --fence={actual}")
            for x in sorted(missing):
                print(f"        落ちた: {x}")
            for x in sorted(added):
                print(f"        増えた: {x}")

# 11b. 多重集合が同じでも、並びと空行は守られていない。
#      宣言済みの増減を両側から取り除いてから、残りを順序込みで比べる。
#      これが無いと「fence 内の 2 行を入れ替える」「空行を挟む」が差分 0 で素通りする。
def _drop_once(seq, items):
    c = Counter(items)
    out = []
    for l in seq:
        if c[l] > 0:
            c[l] -= 1
            continue
        out.append(l)
    return out

if old_code and "--no-fence" not in FLAGS:
    _o = _drop_once(fence_seq(old_body), missing)
    _n = _drop_once(fence_seq(body), added)
    _diff = next(((k, a, b) for k, (a, b) in enumerate(zip(_o, _n)) if a != b), None)
    report("11b", _o == _n,
           f"fence の並びと空行（宣言済みの増減を除いた残り 旧 {len(_o)} 行 / 新 {len(_n)} 行）: "
           + ("一致" if _o == _n
              else f"最初の食い違いは {_diff[0] + 1} 行目 旧 {_diff[1]!r} / 新 {_diff[2]!r}"
                   if _diff else f"行数が違う（旧 {len(_o)} / 新 {len(_n)}）"))

# 12. round 2 で復元した逐語が生きているか。
#     これは手で作った列挙である（機械導出ではない）。母数を宣言する。
# 12. レビューの修正ラウンドで復元・追加した逐語が生きているか。
#     ファイルごとに違うので引数で受ける。手で作った列挙であることを出力に明記する。
ANCHORS = [f.split("=", 1)[1] for f in sorted(FLAGS) if f.startswith("--anchor=")]
# --no-anchors だけは母数が操作者由来（--anchor= の個数）なので、渡すだけで検査が死ぬ。
# 母数を履歴由来にする —— 修正ラウンドが 1 度でも回っていたら、守る逐語が在るはず。
# 母数は「REL を触ったコミット」に絞る。枝の全コミットを数えると、REL を触らない
# コミット（器の導入など）だけで ROUNDS が増え、--no-anchors が成立しなくなる。
# 逆向きも同根で、非空虚ガードの比較先が REL の旧版になり、新版のどの語でも通ってしまう。
_rounds = subprocess.run(["git", "-C", ROOT, "rev-list", "--count", "origin/main..HEAD", "--", REL],
                         capture_output=True, text=True)
ROUNDS = int(_rounds.stdout.strip() or 0) if _rounds.returncode == 0 else 0
if not ANCHORS:
    if "--no-anchors" in FLAGS and ROUNDS > 1:
        die("12", f"--no-anchors を渡したが、origin/main から {ROUNDS} コミット進んでいる。"
                  f"修正ラウンドが回っているなら、復元・追加した逐語を --anchor= で守ること")
    elif "--no-anchors" in FLAGS:
        print(f"SKIP [12] 守る逐語の指定が無い（--no-anchors で明示的に外した。origin/main から {ROUNDS} コミット）")
    else:
        die("12", "守る逐語（--anchor=<逐語>）が 1 件も無い。母数 0 は合格ではない。"
                  "修正ラウンドで復元・追加した記述が無いなら --no-anchors を渡す")
elif "--no-anchors" in FLAGS:
    die("12", f"--no-anchors を渡したが --anchor= が {len(ANCHORS)} 件ある。外す理由が成立していない")
else:
    # 母数を履歴由来にしても、アンカーの中身が操作者由来のままだと
    # 自明に在る語を 1 つ渡すだけで満たせる（--anchor=の で全合格になった）。
    # 守る対象は「修正ラウンドで復元・追加した逐語」なので、その性質も履歴から導く。
    _revs = subprocess.run(["git", "-C", ROOT, "rev-list", "origin/main..HEAD", "--", REL],
                           capture_output=True, text=True).stdout.split()
    vacuous = []
    if _revs:
        # 直前の _rounds は returncode を見ているのに、ここだけ見ていなかった（非対称）。
        # 失敗時に空文字になると全アンカーが素通りするので、揃えて FATAL にする。
        _b = subprocess.run(["git", "-C", ROOT, "show", f"{_revs[-1]}:{REL}"],
                            capture_output=True, text=True)
        if _b.returncode != 0:
            die("12", f"枝の初回コミット {_revs[-1][:7]} の {REL} を取得できない: {_b.stderr.strip()}")
        vacuous = [a for a in ANCHORS if a in _b.stdout]
    else:
        # 枝が origin/main と同じなら、守る対象の履歴が存在しない。
        die("12", "origin/main からのコミットが 0 本。--anchor= を判定する材料が無い")
    if vacuous:
        die("12", f"枝の初回コミットに既に在る逐語をアンカーに渡している（守るものが無い）: {vacuous}")
    else:
        lostA = [a for a in ANCHORS if a not in body]
        report("12", not lostA, f"守る逐語 {len(ANCHORS)} 件（手で列挙。中身は枝の初回コミットに"
                                f"無いことを確かめた）のうち欠けたもの {len(lostA)} 件 {lostA}")

print()
print("注意: このスクリプトは規範項目の欠落を測らない。見出しと見出しの間の散文が 1 本消えても")
print("      全検査が緑になる。「全合格」を「欠落 0 件」の根拠に使わないこと。")
print()
if fatal:
    print(f"FATAL {len(fatal)} 件: {fatal} —— 測定できていない。不合格の証拠にはならない")
    sys.exit(2)
if ng:
    print(f"NG {len(ng)} 件: {ng}")
    sys.exit(1)
print("全合格")
sys.exit(0)
