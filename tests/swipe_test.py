#!/usr/bin/env python3
"""手势实测:文件页右→左滑返回上级;图片查看器左滑下一张 / 右滑上一张"""
import re, time, base64
from playwright.sync_api import sync_playwright

BASE = "http://127.0.0.1:18098"
PNG = base64.b64decode(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=="
)

with sync_playwright() as p:
    b = p.chromium.launch()
    pg = b.new_page(viewport={"width": 390, "height": 844}, has_touch=True, is_mobile=True)
    pg.goto(BASE)
    pg.wait_for_selector(".login-card", timeout=8000)
    cap = pg.evaluate("fetch('/api/captcha').then(r => r.json())")
    code = "".join(re.findall(r">([^<]+)</text>", cap["svg"]))
    pg.evaluate(f"document.getElementById('cap-id').value = {cap['id']!r}")
    pg.fill("#in-user", "admin")
    pg.fill("#in-pass", "admin123")
    pg.fill("#in-captcha", code)
    pg.click("#btn-login")
    pg.wait_for_selector("#files", timeout=8000)
    time.sleep(1.0)

    cdp = pg.context.new_cdp_session(pg)
    def swipe(x1, y1, x2, y2, steps=8):
        cdp.send("Input.dispatchTouchEvent", {"type": "touchStart", "touchPoints": [{"x": x1, "y": y1}]})
        for i in range(1, steps + 1):
            x = x1 + (x2 - x1) * i / steps
            y = y1 + (y2 - y1) * i / steps
            cdp.send("Input.dispatchTouchEvent", {"type": "touchMove", "touchPoints": [{"x": x, "y": y}]})
        cdp.send("Input.dispatchTouchEvent", {"type": "touchEnd", "touchPoints": []})

    def crumbs():
        return pg.locator(".toolbar .crumb a").all_inner_texts()

    pg.evaluate("""window.__evts = [];
      ['pointerdown','pointermove','pointerup','pointercancel'].forEach(t =>
        document.addEventListener(t, e => window.__evts.push(t + ':' + e.pointerType + ':' + Math.round(e.clientX)), true));""")

    # 1. 进入 pics 子目录(轻点文件夹磁贴)
    pg.locator('.tile.dir[data-name="pics"]').tap()
    time.sleep(1.0)
    print("进入子目录, 面包屑:", crumbs())
    pg.screenshot(path="/tmp/s1_subdir.png")

    # 2. 右→左滑 = 返回上级
    swipe(340, 420, 60, 420)
    time.sleep(1.0)
    print("事件探针:", pg.evaluate("window.__evts.slice(0, 20)"))
    print("左滑后, 面包屑:", crumbs())
    pg.screenshot(path="/tmp/s2_after_up.png")

    # 3. 再进入 pics 打开第一张图
    pg.locator('.tile.dir[data-name="pics"]').tap()
    time.sleep(0.8)
    pg.locator('.tile[data-name="a.png"]').tap()
    time.sleep(1.0)
    cnt = pg.locator(".iv-count").inner_text()
    print("查看器打开, 计数:", cnt)

    # 4. 左滑 = 下一张
    swipe(300, 400, 80, 400)
    time.sleep(0.6)
    print("左滑后计数:", pg.locator(".iv-count").inner_text())
    pg.screenshot(path="/tmp/s3_next.png")

    # 5. 右滑 = 上一张
    swipe(80, 400, 300, 400)
    time.sleep(0.6)
    print("右滑后计数:", pg.locator(".iv-count").inner_text())
    pg.screenshot(path="/tmp/s4_prev.png")

    # 6. 纵向滑动不应切图(滚动为主)
    swipe(200, 300, 200, 700)
    time.sleep(0.6)
    print("纵向滑动后计数(应不变):", pg.locator(".iv-count").inner_text())

    b.close()
print("DONE")
