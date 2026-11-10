#!/usr/bin/env python3
"""Build the docs in all five languages.

    python docs/tools/build.py            # en -> docs/book, zh/ja/ko/th -> docs/book/<lang>
    python docs/tools/build.py --check    # also run tools/l10n.py check-all first

English is built first: mdBook empties its build-dir, and the translations
live inside the English one (docs/book/zh, ...). custom.css and lang.js are
shared and copied into each translation's book root before it builds.
"""
import os, shutil, subprocess, sys

HERE = os.path.dirname(os.path.abspath(__file__))
DOCS = os.path.dirname(HERE)
LANGS = ["zh", "ja", "ko", "th"]


def sh(cmd, cwd):
    print("$", " ".join(cmd), f"  (in {os.path.relpath(cwd, DOCS) or '.'})")
    subprocess.run(cmd, cwd=cwd, check=True)


def main():
    if "--check" in sys.argv:
        sh([sys.executable, os.path.join(HERE, "l10n.py"), "check-all"], DOCS)
    sh(["mdbook", "build"], DOCS)
    for l in LANGS:
        root = os.path.join(DOCS, "i18n", l)
        for f in ("custom.css", "lang.js"):
            shutil.copyfile(os.path.join(DOCS, f), os.path.join(root, f))
        sh(["mdbook", "build"], root)
    print("built: docs/book (en) + " + ", ".join(f"docs/book/{l}" for l in LANGS))


if __name__ == "__main__":
    main()
