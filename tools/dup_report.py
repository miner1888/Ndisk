"""同名多版本包深度分析:每个版本被谁依赖 + 我们的直接依赖链。"""
import collections
import pathlib
import re

root = pathlib.Path(__file__).resolve().parent.parent
lock = (root / "Cargo.lock").read_text(encoding="utf-8")

pkgs = {}          # (name, ver) -> deps list
order = []
cur = {}
in_deps = False
for line in lock.splitlines():
    s = line.strip()
    if s == "[[package]]":
        if cur.get("name"):
            pkgs[(cur["name"], cur.get("ver", "?"))] = cur.get("deps", [])
            order.append((cur["name"], cur.get("ver", "?")))
        cur = {}
    elif s.startswith("name = ") and '"' in s:
        cur["name"] = s.split('"')[1]
    elif s.startswith("version = ") and '"' in s:
        cur["ver"] = s.split('"')[1]
    elif s.startswith("dependencies = ["):
        in_deps = True
        cur["deps"] = re.findall(r'"([^"]+)"', s)
    elif s.startswith("]"):
        in_deps = False
    elif in_deps:
        cur["deps"] += re.findall(r'"([^"]+)"', s)
if cur.get("name"):
    pkgs[(cur["name"], cur.get("ver", "?"))] = cur.get("deps", [])
    order.append((cur["name"], cur.get("ver", "?")))

by_name = collections.defaultdict(list)
for (n, v) in order:
    by_name[n].append(v)

# 依赖条目 "name" 或 "name version" -> 匹配到具体 (name, ver)
def resolve(entry):
    parts = entry.split()
    if len(parts) == 1:
        cands = by_name.get(parts[0], [])
        if len(cands) == 1:
            return (parts[0], cands[0])
        return None  # 无版本且歧义,忽略
    return (parts[0], parts[1])

# 反向图: (name,ver) -> set of (parent name, parent ver)
rev = collections.defaultdict(set)
for (n, v), deps in pkgs.items():
    for d in deps:
        r = resolve(d)
        if r:
            rev[r].add((n, v))

multi = {n: vs for n, vs in by_name.items() if len(vs) > 1}
print("=== %d multi-version packages ===" % len(multi))
for n in sorted(multi):
    print("\n[%s]  versions: %s" % (n, ", ".join(sorted(multi[n]))))
    for v in sorted(multi[n]):
        parents = rev.get((n, v), set())
        roots = [p for p in parents if p[0] in
                 ("ndisk",) or (n, v) == ("ndisk", "?")]
        # 我们的 crate 名
        ours = sorted(p for p in parents if p[0] == "ndisk")
        others = sorted(p for p in parents if p[0] != "ndisk")
        if ours:
            print("  v%s  <-- USED DIRECTLY BY OUR CRATE" % v)
        for p in others[:18]:
            print("  v%s  <- %s v%s" % (v, p[0], p[1]))
        if len(others) > 18:
            print("  v%s  <- ... %d more parents" % (v, len(others) - 18))
        if not parents:
            print("  v%s  <- (no parent found / root)" % v)
