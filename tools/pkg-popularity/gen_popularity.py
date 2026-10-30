# gen-popularity.sh's worker: popcon by_inst.gz + pkgstats pages + aliases +
# a nixpkgs index -> crates/daemon/assets/pkg-popularity.tsv
# (attr <TAB> share in parts per 10 000 <TAB> the alias it is also found by).
import glob, gzip, json, sys, time

popcon_gz, stats_dir, aliases_path, index_path, out_path = sys.argv[1:6]

popcon = {}
with gzip.open(popcon_gz, "rt", encoding="utf-8", errors="replace") as f:
    for line in f:
        if line.startswith("#") or not line.strip():
            continue
        parts = line.split()
        if len(parts) < 3 or not parts[2].isdigit() or parts[1] == "Total":  # (its last line sums them)
            continue
        popcon[parts[1]] = int(parts[2])
popcon_machines = popcon["apt"]  # every reporting machine has apt: the count of machines

arch, arch_machines = {}, 1
for path in glob.glob(f"{stats_dir}/pkgstats-*.json"):
    for p in json.load(open(path))["packagePopularities"]:
        arch[p["name"]] = p["count"]
        arch_machines = max(arch_machines, p["samples"])

aliases = {}
others = {}  # "!name": that Debian/Arch name is another program
for line in open(aliases_path, encoding="utf-8"):
    if line.startswith("#") or not line.strip():
        continue
    attr, *names = line.rstrip("\n").split("\t")
    aliases[attr] = [n for n in names if n and not n.startswith("!")]
    others[attr] = {n[1:] for n in names if n.startswith("!")}

index = []
with open(index_path, encoding="utf-8") as f:
    header = f.readline()
    for line in f:
        cols = line.rstrip("\n").split("\t")
        if len(cols) >= 3:
            index.append((cols[0], cols[2]))
attrs = {attr for attr, _ in index}

# Every Debian/Arch name goes to ONE package, the best claim on it: the curated
# alias first, then the package whose attr (a KDE app's tail) is that name, then
# one whose pname is (a pname that is another attr of its own is that package's:
# python313's "python3"). "code" is vscode's (alias), not the coding CLI whose
# attr is "code"; "zed" is zed-editor's, not spicedb-zed's (pname "zed").
claims = {}
def claim(name, attr, rank):
    if not name:
        return
    for n in {name, name.replace("_", "-")} - others.get(attr, set()):
        best = claims.get(n)
        if best is None or rank < best[0]:
            claims[n] = (rank, attr)
for attr, pname in index:
    tail = attr.split(".", 1)[1] if attr.startswith("kdePackages.") else attr
    for n in aliases.get(attr, []):
        claim(n, attr, 0)
    claim(attr, attr, 1)
    claim(tail, attr, 1)
    if pname == attr or pname not in attrs:
        claim(pname, attr, 2)
owned = {}
for name, (_, attr) in claims.items():
    owned.setdefault(attr, []).append(name)

rows = []
for attr, pname in index:
        names = owned.get(attr, [])
        best = max(
            [max(popcon.get(n, 0) / popcon_machines, arch.get(n, 0) / arch_machines) for n in names] or [0]
        )
        share = round(best * 10000)
        if share < 5:  # under 0.05 % of machines: not known
            continue
        tail = attr.split(".", 1)[1] if attr.startswith("kdePackages.") else attr
        also = next((n for n in aliases.get(attr, []) if n not in (attr, pname, tail)), "")
        rows.append((attr, share, also))

rows.sort(key=lambda r: (-r[1], r[0]))
with open(out_path + ".tmp", "w", encoding="utf-8") as f:
    f.write(f"golem-pkg-popularity-v1\t{time.strftime('%Y-%m-%d')}\tdebian-popcon:{popcon_machines}\tarch-pkgstats:{arch_machines}\n")
    for attr, share, also in rows:
        f.write(f"{attr}\t{share}\t{also}\n")
import os
os.replace(out_path + ".tmp", out_path)
print(f">> wrote {out_path} ({len(rows)} packages known)", file=sys.stderr)
