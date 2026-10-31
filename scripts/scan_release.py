#!/usr/bin/env python3
"""Private-data scan over a release: photo file names, secret-shaped tokens and user
paths in every commit since the last tag (its added lines and its message) and in the
working tree's pending diff (tracked changes plus untracked files that will be committed).
Prints per-commit counts and every hit; exit 1 on any hit.

Token shapes (default): a known credential prefix at a word start — `ghp_`, `gho_`,
`ghs_`, `ghu_`, `github_pat_`, `xoxa-`/`xoxb-`/`xoxp-`, `AIza`, `AKIA`, `sk-`, `hf_` —
followed by 16+ key characters; or a 40+ character run of `[A-Za-z0-9+/=]` (no `-` or
`_`, so hyphenated or snake_case identifiers never qualify) that mixes upper case, lower
case and digits and is not an alphabet literal (six or more consecutive characters in
code-point order). `--strict` reports the old broad shape as well: any 32+ character run
of `[A-Za-z0-9_+/=-]` mixing the three classes, and any prefix followed by 10+ characters.

The patterns below are assembled from pieces so this file does not report itself.

usage: scan_release.py [--since <tag>] [--strict] [<extra message file> ...]
       the tree is the repository this script sits in, or $REL_TREE."""
import os
import re
import subprocess
import sys
from pathlib import Path

REPO = os.environ.get("REL_TREE") or str(Path(__file__).resolve().parent.parent)
STRICT = "--strict" in sys.argv

PHOTO = re.compile("_?" + "D" + "SC|A7" + "R0|Wei" + "xin|IM" + "G_")
USERPATH = re.compile("sky" + "ma[^n]|C:" + r"\\\\" + "Users|C:" + r"\\" + "Users|/c" + "/Users|App" + "Data")

_PREFIXES = "(?:gh[posu]_|github_pat_|xox[abp]-|AI" + "za|AK" + "IA|sk-|hf_)"
PREFIX = re.compile(r"(?<![A-Za-z0-9])" + _PREFIXES + r"[A-Za-z0-9_\-]{16,}")
RUN = re.compile(r"(?<![A-Za-z0-9+/=_\-])[A-Za-z0-9+/=]{40,}(?![A-Za-z0-9+/=_\-])")
PREFIX_STRICT = re.compile(_PREFIXES + r"[A-Za-z0-9_\-]{10,}")
RUN_STRICT = re.compile(r"[A-Za-z0-9_+/=\-]{32,}")


def mixed(s):
    return bool(re.search(r"[A-Z]", s) and re.search(r"[a-z]", s) and re.search(r"[0-9]", s))


def alphabet_literal(s):
    step = 1
    for a, b in zip(s, s[1:]):
        step = step + 1 if ord(b) == ord(a) + 1 else 1
        if step >= 6:
            return True
    return False


def token_hits(line):
    hits = PREFIX.findall(line)
    hits += [r for r in RUN.findall(line) if mixed(r) and not alphabet_literal(r)]
    if STRICT:
        hits += [h for h in PREFIX_STRICT.findall(line) if h not in hits]
        hits += [r for r in RUN_STRICT.findall(line) if mixed(r) and r not in hits]
    return hits


def git(*args):
    return subprocess.run(["git", *args], cwd=REPO, capture_output=True, check=True).stdout.decode("utf-8", "replace")


def added(diff):
    return [l[1:] for l in diff.splitlines() if l.startswith("+") and not l.startswith("+++")]


def scan(label, lines, message=""):
    counts = [0, 0, 0]
    for l in lines + message.splitlines():
        for i, hits in enumerate((PHOTO.findall(l), token_hits(l), USERPATH.findall(l))):
            if hits:
                counts[i] += len(hits)
                print(f"   HIT {('photo', 'token', 'userpath')[i]} in {label}: {l.strip()[:160]}")
    print(f"{label}: photo {counts[0]} / token-shape {counts[1]} / user-path {counts[2]}")
    return counts


def main():
    args = sys.argv[1:]
    since = None
    if "--since" in args:
        i = args.index("--since")
        since = args[i + 1]
        del args[i:i + 2]
    args = [a for a in args if a != "--strict"]
    since = since or git("describe", "--tags", "--abbrev=0").strip()
    total = [0, 0, 0]
    shas = git("rev-list", "--reverse", f"{since}..HEAD").split()
    for sha in shas:
        c = scan(sha[:7], added(git("show", "--format=", sha)), git("log", "-1", "--format=%B", sha))
        total = [a + b for a, b in zip(total, c)]
    pending = added(git("diff", "HEAD"))
    for path in git("ls-files", "--others", "--exclude-standard").split("\n"):
        if path:
            pending += open(f"{REPO}/{path}", encoding="utf-8", errors="replace").read().splitlines()
    msgs = "".join(open(extra, encoding="utf-8").read() + "\n" for extra in args)
    c = scan("pending (working tree + untracked + messages)", pending, msgs)
    total = [a + b for a, b in zip(total, c)]
    secrets = [p for p in git("ls-files").split("\n") if re.search(r"(^|/)\.secret", p)]
    print(f"tracked .secret files: {secrets or 'none'}")
    print(f"{since}..HEAD, {len(shas)} commits{' (strict)' if STRICT else ''}: "
          f"photo {total[0]} / token-shape {total[1]} / user-path {total[2]}")
    sys.exit(1 if sum(total) or secrets else 0)


main()
