#!/usr/bin/env python3
"""旧版の文が、新版か退避先に痕跡を残しているかを測る。

使い方: python3 survive.py <repo> <定義ファイルの repo 相対パス> [退避先] [--base=<ref>] [--new=<ref>]

--base は旧版を取る ref（既定 origin/main）。--new を渡すとその ref の新版と比べる（検算用）。

co-review-r2 の手順とは実装を変えてある（同じ道具を 2 者で使えば一致は独立の裏付けに
ならない。台帳 #172）。あちらは絞らずに窓を滑らせる。こちらは位置で絞ってから
n-gram の集合差を取る。

⚠️ 文末の様相（命令形・義務表現）で絞ってはいけない。PR #95 / #96 で落ちた欠落 3 件は
いずれも帰結・理由を述べる文で、命令形でも義務表現でもなかった。絞るのは位置だけ。
"""
import re
import subprocess
import sys
import unicodedata

ROOT, REL = sys.argv[1], sys.argv[2]
_pos = [a for a in sys.argv[3:] if not a.startswith("--")]
_flg = {a.split("=", 1)[0]: a.split("=", 1)[1] for a in sys.argv[3:] if a.startswith("--") and "=" in a}
CASES = _pos[0] if _pos else None
BASE = _flg.get("--base", "origin/main")
NEWREF = _flg.get("--new")

DROP = re.compile(r"[\s*`「」『』【】>|#\-—:：、。（）()\[\]/…🔴⚠️★✅❌]")
NOISE = re.compile(r"20[0-9][0-9]-[0-9][0-9]-[0-9][0-9]|台帳 *#[0-9]|群 [A-Za-zΑ-Ωα-ω]|§[0-9]|stage[0-9]|PR #[0-9]|[0-9a-f]{7,40}")


def norm(s):
    return DROP.sub("", unicodedata.normalize("NFKC", s))


def sentences(text):
    """位置で絞る。見出し・表の罫線・code fence を落とす。文末の形では絞らない。"""
    out, inside = [], False
    for i, line in enumerate(text.split("\n"), 1):
        if line.startswith("```"):
            inside = not inside
            continue
        if inside or not line.strip():
            continue
        if re.match(r"^#{1,6} ", line) or re.match(r"^\|[\s|:-]+\|$", line) or line.strip() in ("---", "‐--"):
            continue
        body = line
        if body.lstrip().startswith("|"):
            cells = [c for c in body.strip().strip("|").split("|")]
        else:
            cells = [body]
        for c in cells:
            for s in re.split(r"(?<=。)", c):
                if len(norm(s)) >= 8:
                    out.append((i, s.strip()))
    return out


def at(ref, path):
    r = subprocess.run(["git", "-C", ROOT, "show", f"{ref}:{path}"], capture_output=True, text=True)
    assert r.returncode == 0, f"{ref}:{path} を取得できない: {r.stderr.strip()}"
    return r.stdout


old = at(BASE, REL)
if NEWREF:
    new = at(NEWREF, REL)
    cases = at(NEWREF, CASES) if CASES else ""
else:
    new = open(f"{ROOT}/{REL}", encoding="utf-8").read()
    cases = open(f"{ROOT}/{CASES}", encoding="utf-8").read() if CASES else ""
B = norm(new) + "\n" + norm(cases)
if norm(old) == norm(new):
    print("FATAL: 旧版と新版が同一。母数が壊れている（--base が書き直し済みの ref を指していないか）")
    sys.exit(2)

sents = sentences(old)
missing, noisy = [], 0
for ln, s in sents:
    n = norm(s)
    # 8-gram をステップ 1 で滑らせる。1 つでも当たれば「言い換えて残った」と見なす。
    if any(n[i:i + 8] in B for i in range(max(1, len(n) - 7))):
        continue
    if NOISE.search(s):
        noisy += 1
        continue
    missing.append((ln, s))

print(f"旧版の文 {len(sents)} 件 / 痕跡が無いもの {len(missing) + noisy} 件")
print(f"  うち日付・群ラベル・sha・章番号を含む（意図的な削除の候補） {noisy} 件 —— 件数だけ出す")
print(f"  残り {len(missing)} 件 —— 全件を出す。**判定して外すなら件数と理由を書くこと**")
print()
print("判定の物差し: 読者の行動を変える文か。変えるなら規範項目（規則本体でも、理由でも、帰結でも、観測点でも）。")
print("              変えないなら経緯・メタ。⚠️ 候補文が乗っている行の台帳項目が『削除』でも、その分類を文へ継承しない。")
print()
for ln, s in missing:
    print(f"  旧 :{ln}  {s[:120]}")
