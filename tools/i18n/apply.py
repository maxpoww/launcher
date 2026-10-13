#!/usr/bin/env python3
"""Wrap the daemon's user-visible literals in i18n::tr (plain ones) or
i18n::fill (format! with holes).  apply.py [--dry] <file.rs>...
Uses scan.py's candidates; the rules for what is NOT a person's text are here."""
import re, sys, os
sys.path.insert(0, os.path.dirname(__file__))
import scan

SKIP_FILES = {"perf.rs", "hypr.rs", "ipc.rs", "launch.rs", "nix.rs", "install.rs", "webapps.rs", "window_memory.rs",
              "nub_drag.rs", "screen.rs", "trash.rs", "unfurl.rs", "font_index.rs", "net.rs", "residue.rs",
              "task_title.rs", "zoom.rs", "applier.rs", "apps.rs", "battery.rs", "action_track.rs", "minimized.rs",
              "i18n.rs", "dict.rs", "dnd.rs", "controls.rs", "content.rs", "sys.rs", "bt.rs", "drives.rs", "groups.rs",
              "deck.rs", "playbox.rs", "emoji_table.rs", "emoji.rs", "logging.rs", "persist.rs", "main.rs", "phones.rs"}
LOWER_OK = {"gear_pages.rs", "gear.rs", "panel.rs", "desktop.rs", "bt_files.rs", "desktop_send.rs"}
SKIP_CALLEE = {"show_control", "open_named", "open_control", "open_content", "is_display_setting", "display_open",
               "module", "read", "p", "kw", "nmcli", "out_of", "adb", "args", "line", "serde", "env", "str"}
SKIP_TEXT = re.compile(r"Pixel 8|^Voice note|Nerd Font|Noto Color|^CTRL|UTF-8$|^Android/|^DCIM$|^Pictures$|"
                       r"^untitled folder|\[\{|->|\{[^{}]*:\?\}|\\n|\\\"|^\(The phone's Configure|^80%, charging|^91 GB|"
                       r"^(English|Español|Português|Français|Deutsch|Italiano|Nederlands|Polski|Svenska|Dansk)\b|"
                       r"^Placeholder for|<[a-z]+>|^Phone$|^Golem shell")

def wanted(c):
    base = os.path.basename(c["file"])
    if c["why"] or base in SKIP_FILES: return False
    t = c["text"]
    if SKIP_TEXT.search(t) or (c["inner"] in SKIP_CALLEE) or (c["callee"] in SKIP_CALLEE): return False
    if t[:1].islower() and base not in LOWER_OK: return False
    if "{{" in t or "}}" in t: return False
    if c["fmt"] or c["holes"]:
        bare = re.sub(r"\{[^{}]*\}", "", t)
        if not c["fmt"] or not re.search(r"[A-Za-z]{2}", bare): return False
        if "=" in t: return False
    return True

def split_args(src, start):
    """The top-level comma-separated pieces of src from `start` up to the
    paren that closes the macro; returns (pieces, index after ')')."""
    depth, i, piece, pieces, n = 0, start, start, [], len(src)
    while i < n:
        ch = src[i]
        if ch == '"':
            i += 1
            while i < n and src[i] != '"': i += 2 if src[i] == "\\" else 1
        elif ch == "'" and re.match(r"'(\\.[^']*|[^'\\])'", src[i:]):
            i += re.match(r"'(\\.[^']*|[^'\\])'", src[i:]).end() - 1
        elif ch in "([{": depth += 1
        elif ch in ")]}":
            if depth == 0:
                pieces.append(src[piece:i].strip()); return [p for p in pieces if p], i + 1
            depth -= 1
        elif ch == "," and depth == 0:
            pieces.append(src[piece:i].strip()); piece = i + 1
        i += 1
    raise ValueError("unclosed macro")

def convert_format(src, c):
    """`format!("…", args)` → `crate::i18n::fill(tr("…"), &[…])`; None if it is not that shape."""
    lit_start, lit_end = c["pos"], c["end"]
    head = re.search(r"format!\s*\(\s*$", src[:lit_start])
    if not head: return None
    args, close = split_args(src, lit_end)
    positional = [a for a in args if not re.match(r"[A-Za-z_][A-Za-z0-9_]*\s*=[^=]", a)]
    named = {m.group(1): m.group(2) for a in args if (m := re.match(r"([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.+)$", a, re.S)) and a not in positional}
    pairs, nxt, used = [], 0, {}
    def hole(m):
        nonlocal nxt
        body = m.group(1)
        name, _, spec = body.partition(":")
        if name == "":
            if nxt >= len(positional): raise ValueError("missing arg")
            key, expr = str(nxt), positional[nxt]; nxt += 1
        elif name.isdigit():
            key, expr = name, positional[int(name)]
        else:
            key, expr = name, named.get(name, name)
        simple = re.fullmatch(r"[A-Za-z_][A-Za-z0-9_.]*(\(\))?", expr)
        value = f'&format!("{{:{spec}}}", {expr})' if spec else (f"&{expr}" if simple else f"&({expr})")
        if key in used and used[key] != value: raise ValueError("one key, two values")
        if key not in used: used[key] = value; pairs.append((key, value))
        return "{" + key + "}"
    try:
        new_lit = re.sub(r"\{([^{}]*)\}", hole, c["text"])
    except (ValueError, IndexError):
        return None
    arr = ", ".join(f'("{k}", {v})' for k, v in pairs)
    return head.start(), close, f'crate::i18n::fill(tr("{new_lit}"), &[{arr}])'

def in_const(src, pos):
    """Whether pos sits in the initializer of a `const`/`static` item."""
    for m in re.finditer(r"(?m)^[ \t]*(pub(\([a-z]+\))? )?(const|static) [A-Z_0-9]+\s*:", src[:pos]):
        last = m
    else:
        pass
    ms = list(re.finditer(r"(?m)^[ \t]*(pub(\([a-z]+\))? )?(const|static) [A-Za-z_0-9]+\s*:", src[:pos]))
    if not ms: return False
    seg = src[ms[-1].start():pos]
    depth = 0
    for tok in scan.tokens(seg):
        if tok[0] == "p" and tok[1] in "([{": depth += 1
        elif tok[0] == "p" and tok[1] in ")]}": depth -= 1
        elif tok[0] == "p" and tok[1] == ";" and depth == 0: return False
    return True

def main():
    dry = "--dry" in sys.argv
    files = [a for a in sys.argv[1:] if not a.startswith("--")]
    total, consts = 0, []
    for path in files:
        src = open(path, encoding="utf-8").read()
        edits = []
        for c in scan.scan(path):
            if not wanted(c): continue
            if in_const(src, c["pos"]) or c.get("static_array"):
                consts.append(f'{path}:{c["line"]}\t{c["text"]}'); continue
            if c["fmt"]:
                conv = convert_format(src, c)
                if conv: edits.append(conv)
                else: consts.append(f'{path}:{c["line"]}\tFMT? {c["text"]}')
            else:
                edits.append((c["pos"], c["end"], "tr(" + src[c["pos"]:c["end"]] + ")"))
        if not edits: continue
        edits.sort()
        out, last = [], 0
        for a, b, new in edits:
            if a < last: continue          # nested in an earlier edit
            out.append(src[last:a]); out.append(new); last = b
        out.append(src[last:])
        new_src = "".join(out)
        if not re.search(r"(?m)^use crate::i18n::(tr|\{[^}]*\btr\b[^}]*\});", new_src):
            uses = list(re.finditer(r"(?m)^use [^;]+;\n", new_src))
            at = uses[-1].end() if uses else 0
            new_src = new_src[:at] + "use crate::i18n::tr;\n" + new_src[at:]
        total += len(edits)
        print(f"{path}: {len(edits)}")
        if not dry:
            tmp = path + ".tmp"; open(tmp, "w", encoding="utf-8").write(new_src); os.replace(tmp, path)
    print(f"total {total}", file=sys.stderr)
    for c in consts: print("CONST\t" + c, file=sys.stderr)

if __name__ == "__main__":
    main()
