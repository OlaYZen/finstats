#!/usr/bin/env python3
"""Build THIRD-PARTY.json, the notice finstats ships for the code it is built on.

Input:  `cargo metadata` for the dependency graph, and the crate sources cargo has already
        unpacked (~/.cargo/registry/src/...), where every crate keeps its own LICENSE files.
Output: one entry per crate — name, version, SPDX expression, repository — and the licence
        texts themselves, deduplicated: hundreds of crates ship the same MIT wording, so the
        texts are listed once each and every crate points at the ones it carries.

    cargo fetch && python3 tools/make-third-party.py

Re-run it whenever a dependency is added, removed or bumped: `cargo test` fails while the file
does not cover every package in Cargo.lock. The bundled fonts and the map are not crates and
are not in here; `src/licenses.rs` adds those from the licence files next to them.
"""
import hashlib
import json
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "THIRD-PARTY.json")

# Files a crate keeps its licence in. NOTICE is Apache-2.0's own attribution file.
LICENCE_FILES = ("LICENSE", "LICENCE", "COPYING", "COPYRIGHT", "NOTICE", "UNLICENSE")
SKIP = (".toml", ".orig", ".md~")
# Some crates ship a LICENSE file holding only "MIT OR Apache-2.0" — a pointer to the two real
# files beside it, not a licence. No licence text is anywhere near this short.
MIN_TEXT = 40


def graph(meta):
    """Every package the binary is built from: the root's dependencies, minus dev-only ones.

    No target filtering: finstats is built for several platforms, and a notice that only covers
    the one it was generated on would be wrong on the others.
    """
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    root = meta["resolve"]["root"]
    seen, stack = set(), [root]
    while stack:
        this = stack.pop()
        if this in seen:
            continue
        seen.add(this)
        for dep in nodes[this]["deps"]:
            if all(k["kind"] == "dev" for k in dep["dep_kinds"]):
                continue
            stack.append(dep["pkg"])
    seen.discard(root)
    return seen


def texts_of(directory):
    """The licence files a crate ships, by name."""
    out = []
    for name in sorted(os.listdir(directory)):
        path = os.path.join(directory, name)
        if not os.path.isfile(path) or not name.upper().startswith(LICENCE_FILES) or name.endswith(SKIP):
            continue
        with open(path, encoding="utf-8", errors="replace") as fh:
            body = fh.read().strip()
        if len(body) >= MIN_TEXT:
            out.append((name, body))
    return out


def main():
    meta = json.loads(subprocess.run(["cargo", "metadata", "--format-version", "1"], cwd=ROOT, check=True, capture_output=True, text=True).stdout)
    packages = {p["id"]: p for p in meta["packages"]}

    notices, index = [], {}
    components, without = [], []
    for pkg_id in graph(meta):
        pkg = packages[pkg_id]
        carried = []
        for name, body in texts_of(os.path.dirname(pkg["manifest_path"])):
            key = hashlib.sha256(body.encode()).hexdigest()
            if key not in index:
                index[key] = len(notices)
                notices.append({"file": name, "text": body})
            if index[key] not in carried:
                carried.append(index[key])
        licence = pkg.get("license") or ""
        if not licence and not carried:
            without.append(pkg["name"])
        components.append({"name": pkg["name"], "version": pkg["version"], "license": licence,
                           "repository": pkg.get("repository") or "", "notices": carried})

    components.sort(key=lambda c: (c["name"].lower(), c["version"]))
    with open(OUT, "w", encoding="utf-8") as fh:
        json.dump({"components": components, "notices": notices}, fh, indent=1, ensure_ascii=False)
        fh.write("\n")
    print(f"{OUT}: {len(components)} components, {len(notices)} licence texts", file=sys.stderr)
    if without:
        print("no licence at all for: " + ", ".join(sorted(set(without))), file=sys.stderr)


if __name__ == "__main__":
    main()
