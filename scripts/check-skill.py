#!/usr/bin/env python3
"""OTA ファームのチェックリスト (skill) が有効か確かめる (CI の build ジョブ、docs: CLAUDE.md)

- .claude/skills/ota-firmware/SKILL.md があり、1 行目が `---`、閉じの `---` があり、その間に `name: ota-firmware`
- CLAUDE.md があり、`.claude/skills/ota-firmware/SKILL.md` を参照している

    scripts/check-skill.py [リポジトリのルート (既定: カレント)]
"""
import os
import sys

SKILL = ".claude/skills/ota-firmware/SKILL.md"
CLAUDE_MD = "CLAUDE.md"


def check(root):
    errors = []
    path = os.path.join(root, SKILL)
    if not os.path.isfile(path):
        errors.append(f"{SKILL} is missing")
    else:
        lines = open(path, encoding="utf-8").read().splitlines()
        if not lines or lines[0].rstrip("\r") != "---":
            errors.append(f"{SKILL}: line 1 must be '---' (YAML frontmatter)")
        else:
            end = next((i for i in range(1, len(lines)) if lines[i].rstrip("\r") == "---"), None)
            if end is None:
                errors.append(f"{SKILL}: frontmatter has no closing '---'")
            else:
                front = lines[1:end]
                if "name: ota-firmware" not in (l.rstrip() for l in front):
                    errors.append(f"{SKILL}: frontmatter has no 'name: ota-firmware'")
                if not any(l.startswith("description:") and l[len("description:"):].strip() for l in front):
                    errors.append(f"{SKILL}: frontmatter has no 'description:'")
    path = os.path.join(root, CLAUDE_MD)
    if not os.path.isfile(path):
        errors.append(f"{CLAUDE_MD} is missing")
    elif SKILL not in open(path, encoding="utf-8").read():
        errors.append(f"{CLAUDE_MD} no longer references {SKILL}")
    return errors


def main():
    errors = check(sys.argv[1] if len(sys.argv) > 1 else ".")
    for e in errors:
        print(f"::error::{e}")
    if not errors:
        print(f"ok: {SKILL} (frontmatter) and {CLAUDE_MD} reference")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
