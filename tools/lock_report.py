"""分析 Cargo.lock:同名多版本包清单 + 直接依赖的版本。"""
import collections
import pathlib
import re
import sys

root = pathlib.Path(__file__).resolve().parent.parent
lock = (root / "Cargo.lock").read_text(encoding="utf-8")

pkgs = []
cur = {}
for line in lock.splitlines():
    line = line.strip()
    if line == "[[package]]":
        if cur:
            pkgs.append(cur)
        cur = {}
    elif line.startswith("name = ") and '"' in line:
        cur["name"] = line.split('"')[1]
    elif line.startswith("version = ") and '"' in line:
        cur["ver"] = line.split('"')[1]
if cur:
    pkgs.append(cur)

print("total package entries:", len(pkgs))
dup = collections.defaultdict(list)
for p in pkgs:
    dup[p.get("name", "?")].append(p.get("ver", "?"))
multi = {k: sorted(v) for k, v in dup.items() if len(v) > 1}
print("packages with MULTIPLE coexisting versions:", len(multi))
for k, v in sorted(multi.items()):
    print("  %-30s %s" % (k, ", ".join("v" + x for x in v)))

# 直接依赖(Cargo.toml [dependencies])及 lock 中实际版本
toml = (root / "Cargo.toml").read_text(encoding="utf-8")
sec = re.search(r"\[dependencies\](.*?)(?=\n\[|\Z)", toml, re.S).group(1)
direct = re.findall(r"^([A-Za-z0-9_-]+)\s*=", sec, re.M)
print("\ndirect dependencies (%d) and locked versions:" % len(direct))
locked = {}
for p in pkgs:
    locked.setdefault(p.get("name", "?"), []).append(p.get("ver", "?"))
for d in sorted(direct):
    vers = locked.get(d, ["(not in lock)"])
    print("  %-28s %s" % (d, ", ".join("v" + x for x in sorted(set(vers)))))
