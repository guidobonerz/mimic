#!/usr/bin/env python3
"""Fetch the VICE test programs the C64 testbench list names into testdata/.

    python tools/fetch_vice_testprogs.py [target-dir]    default: <repo>/testdata

Downloads `testprogs/testbench/c64-testlist.in` and every directory it
refers to (the files directly in it plus its `references/` screenshots) from
the VICE Subversion repository, pinned to one revision, into
`<target-dir>/vice-testprogs/`. The test programs come under their authors'
various licences: run only, never committed. Stdlib only; files already
downloaded are kept, so re-running is cheap.

Source: https://sourceforge.net/p/vice-emu/code/HEAD/tree/testprogs/
"""

import concurrent.futures
import html.parser
import os
import posixpath
import sys
import urllib.error
import urllib.parse
import urllib.request

REVISION = 46303
BASE = f"https://svn.code.sf.net/p/vice-emu/code/!svn/bc/{REVISION}/testprogs/"
TESTLIST = "testbench/c64-testlist.in"


class Listing(html.parser.HTMLParser):
    """Entries of an SVN directory page: names, directories end in '/'."""

    def __init__(self):
        super().__init__()
        self.entries = []

    def handle_starttag(self, tag, attrs):
        if tag != "a":
            return
        href = dict(attrs).get("href", "")
        if href and not href.startswith(("..", "/", "http", "?", "#")):
            self.entries.append(urllib.parse.unquote(href))


def get(url):
    for attempt in range(3):
        try:
            with urllib.request.urlopen(url, timeout=60) as r:
                return r.read()
        except urllib.error.HTTPError as e:
            if e.code == 404:
                return None
            if attempt == 2:
                raise
        except OSError:
            if attempt == 2:
                raise
    return None


def url_of(path):
    return BASE + urllib.parse.quote(path)


def listing(path):
    page = get(url_of(path.rstrip("/") + "/"))
    if page is None:
        return []
    p = Listing()
    p.feed(page.decode("utf-8", "replace"))
    return p.entries


def test_dirs(testlist):
    """Directories (relative to testprogs/) the list's entries live in."""
    dirs = set()
    for line in testlist.splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        d = line.split(",")[0]
        d = posixpath.normpath(posixpath.join("testbench", d))
        if not d.startswith(".."):
            dirs.add(d)
    return sorted(dirs)


def fetch_file(dest, path):
    out = os.path.join(dest, *path.split("/"))
    if os.path.exists(out) and os.path.getsize(out) > 0:
        return False
    data = get(url_of(path))
    if data is None:
        return False
    os.makedirs(os.path.dirname(out), exist_ok=True)
    with open(out + ".part", "wb") as f:
        f.write(data)
    os.replace(out + ".part", out)
    return True


def main():
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    dest = os.path.join(sys.argv[1] if len(sys.argv) > 1 else os.path.join(root, "testdata"),
                        "vice-testprogs")
    os.makedirs(dest, exist_ok=True)
    print(f"VICE test programs (r{REVISION}) -> {dest}")
    fetch_file(dest, TESTLIST)
    with open(os.path.join(dest, *TESTLIST.split("/")), encoding="utf-8") as f:
        dirs = test_dirs(f.read())

    with concurrent.futures.ThreadPoolExecutor(8) as pool:
        listings = dict(zip(dirs, pool.map(listing, dirs)))
        files = []
        refs = []
        for d, entries in listings.items():
            files += [f"{d}/{e}" for e in entries if not e.endswith("/")]
            if "references/" in entries:
                refs.append(f"{d}/references")
        for d, entries in zip(refs, pool.map(listing, refs)):
            files += [f"{d}/{e}" for e in entries if not e.endswith("/")]
        new = sum(pool.map(lambda p: fetch_file(dest, p), files))
    with open(os.path.join(dest, "REVISION"), "w") as f:
        f.write(f"{REVISION}\n")
    print(f"{len(dirs)} directories, {len(files)} files ({new} new)")


if __name__ == "__main__":
    main()
