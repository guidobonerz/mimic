"""Build the mimic user guide as PDFs, in English and German.

    python tools/user_guide.py [docs]        # -> docs/mimic-user-guide-en.pdf, -de.pdf
    python tools/user_guide.py --lang de     # one language only

Needs reportlab (pip install reportlab). guide_lib.py holds fonts, styles and
helpers, guide_en.py and guide_de.py the text (keep them in step with each
other, with the frontends' --help texts, the web page and the machine options
in CLAUDE.md). The logo is `Futuristic MIMIC Tech Emblem.png` in the
repository root. Both PDFs ship with every release package and the web
bundle.
"""

import importlib
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import guide_lib as g  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
LANGUAGES = ["en", "de"]


def build(lang, out_dir):
    g.reset()
    g.front_matter(lang)
    importlib.import_module("guide_" + lang)
    out = os.path.join(out_dir, "mimic-user-guide-%s.pdf" % lang)
    g.Doc(out, lang).multiBuild(list(g.story))
    print("wrote", out)


def main():
    args = sys.argv[1:]
    langs = LANGUAGES
    if "--lang" in args:
        i = args.index("--lang")
        langs = [args[i + 1]]
        del args[i:i + 2]
    out_dir = args[0] if args else os.path.join(ROOT, "docs")
    for lang in langs:
        build(lang, out_dir)


if __name__ == "__main__":
    main()
