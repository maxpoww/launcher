#!/usr/bin/env python3
"""The catalogue's keys: every string the daemon hands the translator.
extract.py [--missing <lang>.json]   (run from the repo root)
Sources: tr("…")/tr_dyn("…") literals; the constant tables translated where
they are drawn (what apply.py leaves raw); tools/i18n/extra-keys.txt."""
import glob, json, os, re, sys
sys.path.insert(0, os.path.dirname(__file__))
import scan, apply

BY_HAND = {"controls.rs", "phones.rs"}

def unescape(s):
    s = re.sub(r"\\\n\s*", "", s)                       # a line continuation
    s = re.sub(r"\\u\{([0-9a-fA-F]+)\}", lambda m: chr(int(m.group(1), 16)), s)
    return s.replace('\\"', '"').replace("\\'", "'").replace("\\n", "\n").replace("\\t", "\t").replace("\\\\", "\\")

def keys():
    found = {}
    src_dir = "crates/daemon/src"
    files = sorted(glob.glob(f"{src_dir}/*.rs") + glob.glob(f"{src_dir}/*/*.rs"))
    for path in files:
        src = open(path, encoding="utf-8").read()
        cut = re.search(r"\n#\[cfg\(test\)\]\s*\n(pub(\([a-z]+\))? )?mod [a-z_]+ \{", src)
        body = src[:cut.start()] if cut else src
        for m in re.finditer(r'\btr(?:_dyn)?\(\s*"((?:[^"\\]|\\.)*)"\s*\)', body):
            found.setdefault(unescape(m.group(1)), path)
        base = os.path.basename(path)
        if base in BY_HAND:
            # Wrapped by hand, often as tr(<a constant or a choice of two>):
            # every literal there that reads as words for a person is a key.
            if base == "phones.rs":      # only what a kind is CALLED; its folders are folders
                for m in re.finditer(r'\blabel:\s*"((?:[^"\\]|\\.)*)"', body):
                    found.setdefault(unescape(m.group(1)), path)
                continue
            for c in scan.scan(path):
                t = c["text"]
                if c["why"] in (None, "done") and not c["fmt"] and not c["holes"] and t[:1].isupper() \
                        and not re.search(r"[,/]|^Adwaita", t) \
                        and not apply.SKIP_TEXT.search(t) and c["inner"] not in apply.SKIP_CALLEE:
                    found.setdefault(unescape(t), path)
            continue
        if base in apply.SKIP_FILES:
            continue
        for c in scan.scan(path):
            table = apply.in_const(src, c["pos"]) or c.get("static_array") or c["inner"] == "guide" or c["callee"] == "guide"
            if not table: continue
            quoted = "\\\"" in c["text"]
            c2 = dict(c); 
            if c["inner"] == "guide" or c["callee"] == "guide":
                if scan.reads_as_words(c["text"]): found.setdefault(unescape(c["text"]), path)
                continue
            if apply.wanted(c) and not c["fmt"]:
                found.setdefault(unescape(c["text"]), path)
    extra = "tools/i18n/extra-keys.txt"
    if os.path.exists(extra):
        for line in open(extra, encoding="utf-8"):
            line = line.rstrip("\n")
            if line and not line.startswith("#"): found.setdefault(unescape(line), extra)
    return found

if __name__ == "__main__":
    k = keys()
    if "--missing" in sys.argv:
        have = json.load(open(sys.argv[sys.argv.index("--missing") + 1], encoding="utf-8"))
        miss = [x for x in sorted(k) if x not in have]
        stale = [x for x in sorted(have) if x not in k]
        for x in miss: print("MISSING\t" + json.dumps(x, ensure_ascii=False))
        for x in stale: print("STALE\t" + json.dumps(x, ensure_ascii=False))
        print(f"{len(k)} keys, {len(miss)} missing, {len(stale)} stale", file=sys.stderr)
        sys.exit(1 if miss else 0)
    json.dump({x: "" for x in sorted(k)}, sys.stdout, ensure_ascii=False, indent=0)
