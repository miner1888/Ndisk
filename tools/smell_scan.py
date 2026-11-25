"""补丁味全局扫描:标记词 + 重复模式统计。"""
import pathlib
import re

SRC = pathlib.Path(r"D:\cs117\ndisk\src")
MARKS = ["TODO", "FIXME", "HACK", "workaround", "临时", "绕过", "特判", "特殊处理",
         "打补丁", "先这样", "没办法", "只能", "兼容性处理", "权宜", "无奈", "旧版行为不同"]
# 合法引用:这些词出现在说明"与旧版一致"的文档性注释里是设计声明而非补丁
OK_CTX = ["与旧版", "旧版一致", "保持一致", "不变量", "语义不变", "契约"]

hits = []
for f in sorted(SRC.rglob("*.rs")):
    rel = f.relative_to(SRC.parent)
    lines = f.read_text(encoding="utf-8").splitlines()
    for i, l in enumerate(lines, 1):
        for m in MARKS:
            if m in l:
                if any(ok in l for ok in OK_CTX):
                    continue
                hits.append("%s:%d: %s" % (rel, i, l.strip()[:120]))
                break

print("标记命中 %d:" % len(hits))
for h in hits:
    print(" ", h)

# 重复模式:同样的长函数签名/结构出现多次(堆叠信号)
print()
print("=== 函数规模 top15(行数,粗略)===")
fns = []
for f in sorted(SRC.rglob("*.rs")):
    rel = f.relative_to(SRC.parent)
    t = f.read_text(encoding="utf-8").splitlines()
    start = None
    name = ""
    for i, l in enumerate(t):
        m = re.match(r"^\s*(pub |pub\(crate\) |pub\(super\) )?(async )?fn (\w+)", l)
        if m and "test" not in l:
            start, name = i, m.group(3)
        elif start is not None and l.strip() == "}":
            fns.append((i - start, "%s:%d %s" % (rel, start + 1, name)))
            start = None
fns.sort(reverse=True)
for n, loc in fns[:15]:
    print("  %4d 行  %s" % (n, loc))
