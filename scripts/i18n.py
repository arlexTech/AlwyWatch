#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""Keeps the translation catalogs in /i18n in step with the code (docs/i18n.md).

  scripts/i18n.py extract     collect every translatable text into i18n/alwywatch.pot and add
                              new ones (untranslated) to each i18n/<lang>.po
  scripts/i18n.py check       fail if the template is stale or a translation breaks a
                              placeholder; print how complete each language is
  scripts/i18n.py new <code> "<Name>" "<Plural-Forms>"
                              start a language, e.g.:
                              new de Deutsch "nplurals=2; plural=(n != 1);"
                              (the Plural-Forms line of any language: see docs/i18n.md)

What counts as translatable: tr("…") and trn("…", "…", …) in Kotlin and Rust (also tr!/trn!),
tr_in!(lang, "…") in the server, and @tr("…") / @tr("…" | "…" % n) in Slint. Only the standard library is used.
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
I18N = ROOT / "i18n"
POT = I18N / "alwywatch.pot"

SOURCES = [
    ("android/app/src/main/java", ".kt", "kotlin"),
    ("core", ".rs", "rust"),
    ("desktop/src", ".rs", "rust"),
    ("web/src", ".rs", "rust"),
    ("desktop/ui", ".slint", "slint"),
    ("server/src", ".rs", "rust"),
]
SKIP = {"core/i18n"}

STR = r'"((?:[^"\\]|\\.|\\\n)*)"'
PATTERNS = {
    "kotlin": [
        re.compile(r"\btrn\(\s*" + STR + r"\s*,\s*" + STR, re.S),
        re.compile(r"\btr\(\s*" + STR, re.S),
    ],
    "rust": [
        re.compile(r"\btrn_in\(\s*[^,()]+,\s*" + STR + r"\s*,\s*" + STR, re.S),
        re.compile(r"\btr_in!\(\s*[^,()]+,\s*" + STR, re.S),
        re.compile(r"\btrn!?\(\s*" + STR + r"\s*,\s*" + STR, re.S),
        re.compile(r"\btr!?\(\s*" + STR, re.S),
    ],
    "slint": [
        re.compile(r"@tr\(\s*" + STR + r"\s*\|\s*" + STR, re.S),
        re.compile(r"@tr\(\s*" + STR, re.S),
    ],
}


def unescape(s, lang):
    out, i = [], 0
    while i < len(s):
        c = s[i]
        if c != "\\":
            out.append(c)
            i += 1
            continue
        n = s[i + 1]
        if n == "\n":  # Rust: backslash-newline joins lines, skipping the indentation.
            i += 2
            while i < len(s) and s[i] in " \t\n":
                i += 1
            continue
        if n == "u" and lang == "rust":
            j = s.index("}", i)
            out.append(chr(int(s[i + 3 : j], 16)))
            i = j + 1
            continue
        if n == "u" and lang == "kotlin":
            out.append(chr(int(s[i + 2 : i + 6], 16)))
            i += 6
            continue
        out.append({"n": "\n", "t": "\t", '"': '"', "\\": "\\", "$": "$", "{": "{", "'": "'"}.get(n, "\\" + n))
        i += 2
    return "".join(out)


def collect():
    """{msgid: {"plural": str|None, "refs": [path:line]}} in first-seen order."""
    found = {}
    for base, ext, lang in SOURCES:
        for path in sorted((ROOT / base).rglob("*" + ext)):
            rel = path.relative_to(ROOT).as_posix()
            if any(rel.startswith(s) for s in SKIP) or "/build/" in rel or "/target/" in rel:
                continue
            text = path.read_text(encoding="utf-8")
            taken = []
            for pat in PATTERNS[lang]:
                for m in pat.finditer(text):
                    if any(a <= m.start() < b for a, b in taken):
                        continue  # the singular of a plural already counted
                    taken.append((m.start(), m.end()))
                    msgid = unescape(m.group(1), lang)
                    plural = unescape(m.group(2), lang) if m.lastindex == 2 else None
                    if not msgid.strip():
                        continue
                    e = found.setdefault(msgid, {"plural": plural, "refs": []})
                    if plural and not e["plural"]:
                        e["plural"] = plural
                    # The file only, no line: moving code must not make the template stale
                    # (CI checks it is up to date).
                    if rel not in e["refs"]:
                        e["refs"].append(rel)
    return found


def quote(s):
    s = s.replace("\\", "\\\\").replace('"', '\\"').replace("\t", "\\t")
    if "\n" not in s:
        return f'"{s}"'
    parts = s.split("\n")
    lines = [p + "\\n" for p in parts[:-1]] + ([parts[-1]] if parts[-1] else [])
    return '""\n' + "\n".join(f'"{l}"' for l in lines)


def parse_po(path):
    """(header lines, {msgid: entry}) with entry = plural, msgstr list, fuzzy, obsolete."""
    entries, header, cur, field = {}, "", None, None
    def done():
        nonlocal cur, header
        if cur is None:
            return
        if cur["msgid"] == "" and not cur["obsolete"]:
            header = cur["msgstr"][0] if cur["msgstr"] else ""
        elif cur["msgid"]:
            entries[cur["msgid"]] = cur
        cur = None
    def new(obsolete=False):
        return {"msgid": "", "plural": None, "msgstr": [], "fuzzy": False, "obsolete": obsolete}
    fuzzy = False
    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        obsolete = line.startswith("#~")
        if obsolete:
            line = line[2:].strip()
        if not line:
            done(); fuzzy = False; continue
        if line.startswith("#,"):
            fuzzy = "fuzzy" in line
            continue
        if line.startswith("#"):
            continue
        m = re.match(r'(msgid_plural|msgid|msgstr(?:\[(\d+)\])?)\s+(".*")$', line)
        if m:
            key, idx, val = m.group(1), m.group(2), unescape(m.group(3)[1:-1], "po")
            if key == "msgid":
                done()
                cur = new(obsolete); cur["fuzzy"] = fuzzy; fuzzy = False
                cur["msgid"] = val; field = ("msgid", None)
            elif key == "msgid_plural":
                cur["plural"] = val; field = ("plural", None)
            else:
                i = int(idx) if idx else 0
                while len(cur["msgstr"]) <= i:
                    cur["msgstr"].append("")
                cur["msgstr"][i] = val; field = ("msgstr", i)
        elif line.startswith('"') and cur is not None:
            val = unescape(line[1:-1], "po")
            if field[0] == "msgid":
                cur["msgid"] += val
            elif field[0] == "plural":
                cur["plural"] += val
            else:
                cur["msgstr"][field[1]] += val
    done()
    return header, entries


def nplurals(header):
    m = re.search(r"nplurals\s*=\s*(\d+)", header)
    return int(m.group(1)) if m else 2


def write_entry(out, msgid, plural, msgstr, refs=(), fuzzy=False, obsolete=False, n=2):
    p = "#~ " if obsolete else ""
    for r in refs[:3]:
        out.append(f"#: {r}")
    if fuzzy:
        out.append("#, fuzzy")
    def field(name, value):
        for i, l in enumerate(quote(value).split("\n")):
            out.append(f"{p}{name} {l}" if i == 0 else f"{p}{l}")
    field("msgid", msgid)
    if plural is not None:
        field("msgid_plural", plural)
        for i in range(n):
            field(f"msgstr[{i}]", msgstr[i] if i < len(msgstr) else "")
    else:
        field("msgstr", msgstr[0] if msgstr else "")
    out.append("")


def pot_text(found):
    out = ['# Template: every translatable text of AlwyWatch. Generated by scripts/i18n.py extract;',
           '# do not edit. Translations live next to it, one <lang>.po per language.',
           'msgid ""', 'msgstr ""', '"Content-Type: text/plain; charset=UTF-8\\n"', ""]
    for msgid, e in found.items():
        write_entry(out, msgid, e["plural"], [], e["refs"])
    return "\n".join(out)


def merge(path, found):
    header, old = parse_po(path)
    n = nplurals(header)
    out = []
    for l in path.read_text(encoding="utf-8").splitlines():
        if l.startswith("#") and not l.startswith("#~") and not l.startswith("#:") and not l.startswith("#,"):
            out.append(l)
        else:
            break
    out += ['msgid ""', 'msgstr ""'] + [f'"{h}\\n"' for h in header.split("\n") if h] + [""]
    for msgid, e in found.items():
        o = old.get(msgid)
        msgstr = o["msgstr"] if o and (o["plural"] is None) == (e["plural"] is None) else []
        write_entry(out, msgid, e["plural"], msgstr, e["refs"], bool(o and o["fuzzy"]), n=n)
    for msgid, o in old.items():
        if msgid not in found and any(o["msgstr"]):
            write_entry(out, msgid, o["plural"], o["msgstr"], obsolete=True, n=n)
    path.write_text("\n".join(out), encoding="utf-8")


PLACEHOLDER = re.compile(r"\{[a-zA-Z0-9_]*\}")


def check(found):
    ok = True
    if not POT.exists() or POT.read_text(encoding="utf-8") != pot_text(found):
        print("i18n/alwywatch.pot is out of date: run scripts/i18n.py extract")
        ok = False
    for po in sorted(I18N.glob("*.po")):
        header, entries = parse_po(po)
        live = [e for k, e in entries.items() if k in found and not e["obsolete"]]
        done = [e for e in live if e["msgstr"] and all(e["msgstr"]) and not e["fuzzy"]]
        for e in done:
            want = set(PLACEHOLDER.findall(e["msgid"]))
            forms = e["msgstr"]
            for i, s in enumerate(forms):
                ref = set(PLACEHOLDER.findall(e["plural"] or e["msgid"])) if i else want
                got = set(PLACEHOLDER.findall(s))
                # A singular form may leave out {n} ("un titre"); nothing else may be lost or made up.
                if got - (want | ref) or (ref - got) - ({"{n}"} if e["plural"] else set()):
                    print(f"{po.name}: placeholders differ in {e['msgid']!r} → {s!r}")
                    ok = False
        pct = 100 * len(done) // max(1, len(found))
        print(f"{po.stem}: {len(done)}/{len(found)} translated ({pct}%)")
    return ok


def main():
    cmd = sys.argv[1] if len(sys.argv) > 1 else "check"
    found = collect()
    if cmd == "extract":
        POT.write_text(pot_text(found), encoding="utf-8")
        for po in sorted(I18N.glob("*.po")):
            merge(po, found)
        print(f"{len(found)} texts")
        check(found)
    elif cmd == "check":
        sys.exit(0 if check(found) else 1)
    elif cmd == "new" and len(sys.argv) == 5:
        code, name, forms = sys.argv[2:5]
        if not re.fullmatch(r"\s*nplurals\s*=\s*\d+\s*;\s*plural\s*=.+;?\s*", forms):
            sys.exit('Plural-Forms looks like "nplurals=2; plural=(n != 1);"')
        forms = forms.strip().rstrip(";") + ";"
        path = I18N / f"{code}.po"
        if path.exists():
            sys.exit(f"{path} exists")
        path.write_text(
            f'# {name} translation of AlwyWatch.\n# SPDX-License-Identifier: AGPL-3.0-or-later\n'
            f'msgid ""\nmsgstr ""\n"Language: {code}\\n"\n"X-Language-Name: {name}\\n"\n'
            f'"MIME-Version: 1.0\\n"\n"Content-Type: text/plain; charset=UTF-8\\n"\n'
            f'"Content-Transfer-Encoding: 8bit\\n"\n"Plural-Forms: {forms}\\n"\n',
            encoding="utf-8",
        )
        merge(path, found)
        print(f"created {path.relative_to(ROOT)}; translate it, then rebuild")
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()
