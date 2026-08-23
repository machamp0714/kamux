#!/usr/bin/env python3
"""台帳（.claude/AGENT-LESSONS.md）の列数検算の正典。

素の `|`（escape されていない列区切り）だけを数え、表ブロックごとにヘッダ行と本数を
突き合わせる。`awk -F'|'` は escape 済み `\\|` も区切りに数えるため、正常な行を NG と
報告する偽陽性を作る（#58 ほか 5 行が 12 日間「壊れた行」として扱われた実例）。

終了コード: 0 = 全ブロック整合 / 1 = 不整合あり / 2 = 前提不正（ファイルが読めない等）。
使い方: python3 check-ledger-columns.py [対象ファイル ...]（省略時は .claude/AGENT-LESSONS.md）
"""
import re
import sys

RAW_PIPE = re.compile(r"(?<!\\)\|")


def check(path: str) -> int:
    try:
        lines = open(path, encoding="utf-8").read().split("\n")
    except OSError as e:
        print(f"FATAL: {path}: {e}")
        return 2
    ng = 0
    i = 0
    while i < len(lines):
        if lines[i].startswith("|"):
            j = i
            while j < len(lines) and lines[j].startswith("|"):
                j += 1
            head = len(RAW_PIPE.findall(lines[i]))
            for k in range(i, j):
                got = len(RAW_PIPE.findall(lines[k]))
                if got != head:
                    ng += 1
                    print(f"NG {path}:{k + 1} 素の '|' が {got} 本（ブロックのヘッダは {head} 本）")
            i = j
        else:
            i += 1
    blocks = "整合" if ng == 0 else f"不整合 {ng} 行"
    print(f"{path}: {blocks}")
    return 0 if ng == 0 else 1


def main() -> int:
    targets = sys.argv[1:] or [".claude/AGENT-LESSONS.md"]
    worst = 0
    for t in targets:
        rc = check(t)
        worst = max(worst, rc)
    return worst


if __name__ == "__main__":
    sys.exit(main())
