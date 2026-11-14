#!/usr/bin/env python3
"""用 logo.svg 渲染各尺寸 RGBA,组装全 BMP 帧的 ICO(rc.exe / windres 兼容)"""
import io, os, struct
from playwright.sync_api import sync_playwright
from PIL import Image

SIZES = [16, 24, 32, 48, 64, 96, 128, 256]
ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
SVG = os.path.join(ROOT, "web", "logo.svg")
OUT = os.path.join(ROOT, "assets", "ndisk.ico")

# 1. Chromium 按 SVG 矢量渲染各尺寸(透明背景)
pngs = {}
with sync_playwright() as p:
    b = p.chromium.launch()
    pg = b.new_page(viewport={"width": 300, "height": 300}, device_scale_factor=1)
    for s in SIZES:
        pg.goto("file://" + os.path.abspath(SVG))
        pg.evaluate(f"""document.documentElement.setAttribute('width','{s}');
                        document.documentElement.setAttribute('height','{s}');""")
        pngs[s] = pg.locator("svg").screenshot(omit_background=True)
        open(f"/tmp/icon-{s}.png", "wb").write(pngs[s])
    b.close()

# 2. 组装 ICO:每帧 BMP(32bpp BGRA 自底向上 + AND 掩码),无 PNG 帧,rc.exe/windres 兼容
frames = []
for s in SIZES:
    w = h = s
    rgba = Image.open(io.BytesIO(pngs[s])).convert("RGBA").tobytes()
    stride = w * 4
    mask_stride = ((w + 31) // 32) * 4
    bmp = struct.pack("<IiiHHIIiiII", 40, w, h * 2, 1, 32, 0, w * h * 4 + mask_stride * h, 0, 0, 0, 0)
    # XOR 像素块:BGRA 自底向上,行间无掩码
    body = bytearray()
    for row in range(h - 1, -1, -1):
        line = rgba[row * stride:(row + 1) * stride]
        for i in range(0, stride, 4):
            body += bytes([line[i + 2], line[i + 1], line[i], line[i + 3]])
    # AND 掩码块:独立的整块(32bpp 有 alpha 时全 0),绝不能逐行插入像素
    body += b"\x00" * (mask_stride * h)
    frames.append((s, bmp + bytes(body)))

ico = struct.pack("<HHH", 0, 1, len(frames))
entries = b""
data = b""
offset = 6 + len(frames) * 16
for s, blob in frames:
    entries += struct.pack("<BBBBHHII", s % 256, s % 256, 0, 0, 1, 32, len(blob), offset)
    data += blob
    offset += len(blob)
open(OUT, "wb").write(ico + entries + data)

# 3. 自检
chk = open(OUT, "rb").read()
cnt = struct.unpack("<H", chk[4:6])[0]
sizes = []
for i in range(cnt):
    off = 6 + i * 16
    w, h, _, _, _, bpp, size, off2 = struct.unpack("<BBBBHHII", chk[off:off + 16])
    w = 256 if w == 0 else w
    assert chk[off2:off2 + 4] != b"\x89PNG", "混入 PNG 帧"
    sizes.append(w)
print(f"已生成 {OUT}({os.path.getsize(OUT)}B,{cnt} 帧全 BMP:{sizes})")
