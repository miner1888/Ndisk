"""打包干净源码备份(无 vendor 版)。

排除规则按**路径锚定**,不是按目录名:
  - 仅项目根下的构建产物 / 运行时数据目录(target / data / uidebug / smoke-data)
  - 任意层级的 VCS 与工具缓存(.git / node_modules / __pycache__)
  - 仅项目根下的临时文件(*.log / *.sqlite3* / *.zip)

打包后自检:扫描全部 .rs 中的字面量 include_bytes!/include_str!,逐个确认目标
文件已进入包内(源树里本来就不存在的目标不计);任何误删都以非零退出码报错。

用法: python -I tools\\pack_src.py
"""
import os
import pathlib
import re
import shutil
import sys
import tempfile
import zipfile

SRC = pathlib.Path(__file__).resolve().parent.parent
STAMP = "20261125"
OUT = SRC.parent / (SRC.name + "-src-%s.zip" % STAMP)
FIXED_TS = 1767225600  # 2026-01-01 00:00:00 UTC,ZIP 允许范围内的固定时间戳

SKIP_TOP_DIRS = {"target", "data", "uidebug", "smoke-data"}
SKIP_ANY_DIRS = {".git", "node_modules", "__pycache__"}
SKIP_TOP_EXT = (".log", ".sqlite3", ".sqlite3-wal", ".sqlite3-shm", ".zip", ".crate")

stage = pathlib.Path(tempfile.mkdtemp(prefix="ndisk-src-"))
dst = stage / SRC.name
count = 0
for root, dirs, files in os.walk(SRC):
    rel_root = pathlib.Path(root).relative_to(SRC)
    at_top = rel_root == pathlib.Path(".")
    kept = []
    for d in dirs:
        if d in SKIP_ANY_DIRS:
            continue
        if at_top and d in SKIP_TOP_DIRS:
            continue
        kept.append(d)
    dirs[:] = kept
    for f in files:
        if at_top and f.endswith(SKIP_TOP_EXT):
            continue
        src = pathlib.Path(root) / f
        target = dst / src.relative_to(SRC)
        try:
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(src, target)
        except OSError:
            print("skip special file:", src)
            continue
        count += 1

# 自检:字面量 include 目标必须在暂存树里(对自己源树里存在的目标判错)
INC = re.compile(r'include_(?:bytes|str)!\s*\(\s*"([^"]+)"\s*\)')
dropped = []
for p in dst.rglob("*.rs"):
    text = p.read_text(encoding="utf-8", errors="replace")
    orig_rs = SRC / p.relative_to(dst)
    for m in INC.finditer(text):
        rel_target = m.group(1)
        if (p.parent / rel_target).exists():
            continue
        if (orig_rs.parent / rel_target).exists():
            dropped.append("%s -> %s" % (p.relative_to(dst), rel_target))

for p in dst.rglob("*"):
    if p.is_file():
        try:
            os.utime(p, (FIXED_TS, FIXED_TS))
        except OSError:
            pass

with zipfile.ZipFile(OUT, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as z:
    for p in sorted(dst.rglob("*")):
        if p.is_file():
            z.write(p, p.relative_to(stage))
shutil.rmtree(stage)

with zipfile.ZipFile(OUT) as z:
    names = [n.replace("\\", "/") for n in z.namelist()]
    rs = sum(1 for n in names if n.endswith(".rs") and "/src/" in n)
    rs_total = sum(1 for n in names if n.endswith(".rs"))
    js = sum(1 for n in names if n.endswith(".js"))

size_mb = OUT.stat().st_size / (1024 * 1024)
print("zip:", OUT)
print("entries: %d  size: %.2f MB  (source files copied: %d)" % (len(names), size_mb, count))
print("rust src: %d (+%d other .rs)  js: %d" % (rs, rs_total - rs, js))

if dropped:
    print("\n!! 打包误删了 %d 个 include 依赖(会导致编译失败):" % len(dropped))
    for m in dropped[:40]:
        print("   ", m)
else:
    print("include 完整性: OK")
sys.exit(1 if dropped else 0)
