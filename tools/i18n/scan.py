#!/usr/bin/env python3
"""Find the daemon's user-visible string literals that do not go through
i18n::tr yet.  scan.py [--json] <file.rs>...   (see tools/i18n/README.md)

A literal is a CANDIDATE when it reads as words for a person (a capital or a
space, at least one letter run of 3) and sits outside tests, comments, log
macros, program arguments, patterns and comparisons."""
import json, re, sys

LOG = {"info", "warn", "debug", "error", "trace", "bail", "anyhow", "panic", "assert", "assert_eq",
       "assert_ne", "debug_assert", "debug_assert_eq", "unreachable", "todo", "unimplemented", "eprintln",
       "println", "print", "eprint", "write", "writeln", "matches", "concat", "include_str", "include_bytes",
       "env", "cfg", "compile_error", "expect", "span", "info_span", "debug_span"}
# calls whose string arguments are never shown to a person
QUIET = {"new", "arg", "args", "var", "var_os", "set_var", "remove_var", "starts_with", "ends_with", "contains",
         "strip_prefix", "strip_suffix", "split", "rsplit", "split_once", "rsplit_once", "trim_start_matches",
         "trim_end_matches", "trim_matches", "find", "rfind", "replace", "replacen", "join", "context",
         "with_context", "expect", "get", "pointer", "contains_key", "insert", "remove", "entry", "name", "parse",
         "from_str", "path", "data_path", "golem_path", "tr", "tr_dyn", "on_path", "tool", "out", "run",
         "eval", "eval_ok", "dispatch", "dispatch_checked", "request", "reply_json", "option_text", "is_some_and",
         "position", "any", "all", "filter", "map_or", "unwrap_or_else", "open", "create", "exists", "extension",
         "set_extension", "with_extension", "file_name", "push_str_raw", "getoption", "send_key_to",
         "send_shortcut_active", "spawn", "builder", "thread_name", "starts", "eq_ignore_ascii_case", "lookup",
         "env_remove", "current_dir", "from_bytes_with_nul", "CString", "c_str", "call", "call_method",
         "interface", "member", "destination", "sender", "path_namespace", "property", "set_namespace",
         "namespace", "class", "title", "debug", "ctl", "verb", "write_json", "write_text", "write_bytes",
         "read_json", "log", "time", "hit", "measure", "key", "utf8", "json", "pointer_mut", "as_str_or"}

def tokens(src):
    """Yield (kind, text, pos). kind: s (string), id, p (punct)."""
    i, n = 0, len(src)
    while i < n:
        c = src[i]
        if c.isspace():
            i += 1
        elif src.startswith("//", i):
            j = src.find("\n", i); i = n if j < 0 else j
        elif src.startswith("/*", i):
            depth, j = 1, i + 2
            while j < n and depth:
                if src.startswith("/*", j): depth += 1; j += 2
                elif src.startswith("*/", j): depth -= 1; j += 2
                else: j += 1
            i = j
        elif c == '"' or (c in "rb" and re.match(r'(br|rb|r|b)#*"', src[i:i+6] or "")):
            m = re.match(r'(br|rb|r|b)?(#*)"', src[i:])
            pre, hashes = m.group(1) or "", m.group(2)
            j = i + m.end()
            if "r" in pre:
                end = '"' + hashes
                k = src.find(end, j); k = n if k < 0 else k
                yield ("s", src[j:k], i, ("raw", k + len(end))); i = k + len(end)
            else:
                k = j
                while k < n and src[k] != '"':
                    k += 2 if src[k] == "\\" else 1
                yield ("s", src[j:k], i, ("b" if "b" in pre else "", k + 1)); i = k + 1
        elif c == "'":
            m = re.match(r"'(\\.[^']*|[^'\\])'", src[i:])
            if m: i += m.end()
            else:
                m = re.match(r"'[A-Za-z_][A-Za-z0-9_]*", src[i:]); i += m.end() if m else 1
        elif c.isalpha() or c == "_":
            m = re.match(r"[A-Za-z_][A-Za-z0-9_]*", src[i:])
            yield ("id", m.group(0), i, None); i += m.end()
        elif c.isdigit():
            m = re.match(r"[0-9][A-Za-z0-9_.]*", src[i:]); i += m.end()
        else:
            two = src[i:i+2]
            if two in ("=>", "==", "!=", "::", "->", "..", "&&", "||"):
                yield ("p", two, i, None); i += 2
            else:
                yield ("p", c, i, None); i += 1

NOISE = re.compile(r"\$\(|&& |2>/dev/null|%s|^(env |busctl|xdg-open|find |echo |cd |getprop|ip -|while read|printf|\\[Service\\]|export |# )|=api\\.|media\\.role|node\\.description")
DBUS = {"get_property", "set_property", "as_bool", "as_string", "as_u32", "as_u64", "spawn_device_call", "adapter_set",
        "receive_signal", "call_method", "call", "s", "prop", "get_all", "bool", "dev_bool", "dev_str"}
WORDY = re.compile(r"[A-Za-z]{3}")
def reads_as_words(s):
    if not WORDY.search(s): return False
    if re.fullmatch(r"[a-z0-9_./:@%+=,\-\[\]{}*#~$^|\\ ]*", s) and " " not in s.strip(): return False   # ids, paths, flags
    if s.startswith(("/", "./", "~/", "--", "$", "hl.", "http", "org.", "j/")): return False
    if re.fullmatch(r"[a-z_]+( [a-z_<>|\[\]{}.-]+)*", s) and not s[:1].isupper() and len(s.split()) <= 2 and not s.endswith((".", "…", "?")):
        return False   # ctl verbs like "open net"
    return s[:1].isupper() or " " in s.strip()

def scan(path):
    src = open(path, encoding="utf-8").read()
    cut = len(src)
    m = re.search(r"\n#\[cfg\(test\)\]\s*\n(pub(\([a-z]+\))? )?mod [a-z_]+ \{", src)
    if m: cut = m.start()
    toks = list(tokens(src[:cut]))
    out, stack = [], []          # stack of (opener, callee, is_macro)
    fns, pending_fn = [], None   # (name, depth of its body)
    opens = []                   # token index of each open bracket on the stack
    for idx, (kind, text, pos, extra) in enumerate(toks):
        if kind == "id" and text == "fn" and idx + 1 < len(toks) and toks[idx+1][0] == "id":
            pending_fn = toks[idx+1][1]
        if kind == "p" and text == ";" and pending_fn and not any(o == "(" for o, _, _ in stack[-1:]):
            pending_fn = None
        if kind == "p" and text == "{" and pending_fn:
            fns.append((pending_fn, len(stack))); pending_fn = None
        if kind == "p" and text == "}" and fns and fns[-1][1] == len(stack) - 1:
            fns.pop()
        if kind == "p" and text in "([{":
            callee, macro = None, False
            if text == "[" and idx >= 1 and toks[idx-1][1] in ("#", "!"):
                callee = "#attr"
            if idx >= 1 and toks[idx-1][0] == "id":
                callee = toks[idx-1][1]
            elif idx >= 2 and toks[idx-1][1] == "!" and toks[idx-2][0] == "id":
                callee, macro = toks[idx-2][1], True
            stack.append((text, callee, macro)); opens.append(idx)
        elif kind == "p" and text in ")]}":
            if stack: stack.pop(); opens.pop()
        elif kind == "s":
            flavour, end = extra
            if flavour in ("raw", "b"): continue
            prev = toks[idx-1] if idx else ("p", "", 0, None)
            nxt = toks[idx+1] if idx + 1 < len(toks) else ("p", "", 0, None)
            callee, macro = None, False
            for op, cal, mac in reversed(stack):
                if cal: callee, macro = cal, mac; break
                if op == "{": break
            inner = stack[-1] if stack else ("", None, False)
            why = None
            fn = fns[-1][0] if fns else ""
            if any(mac and cal in LOG for _, cal, mac in stack): why = "log"
            elif any(cal == "#attr" for _, cal, _ in stack): why = "attr"
            elif re.search(r"debug|_command$|^parse_|^fmt$", fn): why = "debugfn"
            elif NOISE.search(text) or re.fullmatch(r"[A-Z][a-z0-9]+([A-Z][a-z0-9]*)+|[A-Z0-9_]{2,}", text.strip()): why = "noise"
            elif inner[1] in DBUS: why = "dbus"
            elif inner[1] in QUIET and not inner[2]: why = "quiet:" + inner[1]
            elif nxt[1] in ("=>", "|") or prev[1] in ("|", "==", "!=") or nxt[1] in ("==", "!="): why = "pattern"
            elif prev[1] == "=" and idx >= 2 and toks[idx-2][0] == "id" and idx >= 4 and toks[idx-4][1] in ("const", "static"): why = "const"
            elif prev[0] == "id" and prev[1] in ("tr", "tr_dyn"): why = "done"
            elif prev[1] == "(" and inner[1] in ("tr", "tr_dyn"): why = "done"
            if not reads_as_words(text): continue
            line = src.count("\n", 0, pos) + 1
            fmt = macro and callee == "format" and inner[1] == "format"
            holes = bool(re.search(r"\{[^{}]*\}", text.replace("{{", "").replace("}}", "")))
            # `&["a", "b"]`: a borrowed array literal lives for 'static only while
            # its elements are constants.
            static_array = False
            for d in range(len(stack) - 1, -1, -1):
                if stack[d][0] == "[":
                    k = opens[d]
                    static_array = k >= 1 and toks[k-1][1] == "&"
                    break
                if stack[d][0] == "{": break
            out.append(dict(static_array=static_array, fn=fn, file=path, line=line, pos=pos, end=end, text=text, why=why, fmt=fmt, holes=holes,
                            callee=callee, inner=inner[1], macro=macro, prev=prev[1], next=nxt[1]))
    return out

if __name__ == "__main__":
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    found = [c for f in args for c in scan(f)]
    if "--json" in sys.argv:
        json.dump(found, sys.stdout)
    else:
        for c in found:
            if c["why"]: continue
            tag = "F" if c["fmt"] else ("H" if c["holes"] else " ")
            print(f'{c["file"]}:{c["line"]}\t{tag}\t{c["callee"] or "-"}\t{c["text"]}')
