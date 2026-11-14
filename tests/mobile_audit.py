#!/usr/bin/env python3
"""移动端全页面截图审查 + 右缘返回手势验证"""
import re, time
from playwright.sync_api import sync_playwright

BASE = "http://127.0.0.1:18098"

with sync_playwright() as p:
    b = p.chromium.launch()
    pg = b.new_page(viewport={"width": 390, "height": 844}, has_touch=True, is_mobile=True)
    pg.goto(BASE)
    pg.wait_for_selector(".login-card", timeout=8000)
    pg.screenshot(path="/tmp/a00_login.png")
    cap = pg.evaluate("fetch('/api/captcha').then(r => r.json())")
    code = "".join(re.findall(r">([^<]+)</text>", cap["svg"]))
    pg.evaluate(f"document.getElementById('cap-id').value = {cap['id']!r}")
    pg.fill("#in-user", "admin"); pg.fill("#in-pass", "admin123"); pg.fill("#in-captcha", code)
    pg.click("#btn-login"); pg.wait_for_selector("#files", timeout=8000); time.sleep(1.0)

    cdp = pg.context.new_cdp_session(pg)
    def swipe(x1, y1, x2, y2, steps=8):
        cdp.send("Input.dispatchTouchEvent", {"type": "touchStart", "touchPoints": [{"x": x1, "y": y1}]})
        for i in range(1, steps + 1):
            cdp.send("Input.dispatchTouchEvent", {"type": "touchMove",
              "touchPoints": [{"x": x1 + (x2 - x1) * i / steps, "y": y1 + (y2 - y1) * i / steps}]})
        cdp.send("Input.dispatchTouchEvent", {"type": "touchEnd", "touchPoints": []})

    # 1. 文件页(网格)
    pg.screenshot(path="/tmp/a01_files.png")
    # 2. 列表视图
    pg.locator("#tb-view").tap(); time.sleep(0.5)
    pg.screenshot(path="/tmp/a02_list.png")
    pg.locator("#tb-view").tap(); time.sleep(0.5)
    # 3. 搜索态 + 右缘返回关闭
    pg.locator("#btn-search").tap(); time.sleep(0.4)
    pg.screenshot(path="/tmp/a03_search.png")
    swipe(380, 400, 200, 400); time.sleep(0.6)
    print("搜索态右缘返回关闭:", pg.locator("#btn-search").is_visible())
    # 4. 进入子目录 → 相册 → 右缘返回文件页
    pg.locator(".tile.dir[data-name='pics']").tap(); time.sleep(0.8)
    pg.locator("#btn-menu").tap(); time.sleep(0.4)
    pg.locator(".share-item[data-album]").tap(); time.sleep(1.2)
    pg.screenshot(path="/tmp/a04_album.png")
    swipe(380, 400, 200, 400); time.sleep(0.8)
    print("相册右缘返回文件页:", pg.locator("#btn-album-back").count() == 0 and pg.locator(".album-grid").count() == 0)
    print("返回后仍在原目录 pics:", "pics" in pg.locator(".toolbar .crumb").inner_text())
    # 5. 打开图片 → 右缘返回(查看器自带)
    if pg.locator(".tile[data-name='a.png']").count() == 0:
        pg.locator(".tile.dir[data-name='pics']").tap(); time.sleep(0.8)
    pg.locator(".tile[data-name='a.png']").tap(); time.sleep(1.0)
    pg.screenshot(path="/tmp/a05_viewer.png")
    swipe(380, 400, 200, 400); time.sleep(0.6)
    print("查看器右缘返回:", pg.locator(".imgviewer").count() == 0)
    # 6. 管理后台 users → 右缘返回
    pg.locator("#btn-menu").tap(); time.sleep(0.4)
    pg.locator("#whoBtn").tap(); time.sleep(0.4)
    pg.locator(".ctx-menu .ctx-item", has_text="管理后台").tap(); time.sleep(1.0)
    pg.screenshot(path="/tmp/a06_admin_users.png")
    pg.evaluate("""window.__evts = [];
      ['pointerdown','pointermove','pointerup','pointercancel'].forEach(t =>
        document.addEventListener(t, e => window.__evts.push(t + ':' + e.pointerType + '@' + Math.round(e.clientX) + ',' + Math.round(e.clientY) + ' ' + (e.target.className && e.target.className.baseVal !== undefined ? 'svg' : (e.target.className || e.target.tagName))), true));""")
    swipe(380, 400, 200, 400); time.sleep(0.8)
    print("后台右缘事件流:", pg.evaluate("window.__evts.slice(0, 6)"))
    print("管理后台右缘返回文件页:", pg.locator(".admin-wrap").count() == 0)
    # 7. 后台各页签截图
    pg.locator("#btn-menu").tap(); time.sleep(0.3)
    pg.locator("#whoBtn").tap(); time.sleep(0.3)
    pg.locator(".ctx-menu .ctx-item", has_text="管理后台").tap(); time.sleep(0.8)
    for pane in ["shares", "maint", "tls", "account"]:
        pg.locator(f'.nav-item[data-pane="{pane}"]').tap(); time.sleep(0.8)
        pg.screenshot(path=f"/tmp/a07_admin_{pane}.png")
    swipe(380, 400, 200, 400); time.sleep(0.8)
    print("TLS 页右缘返回文件页:", pg.locator(".admin-wrap").count() == 0)
    if pg.locator(".imgviewer").count():
        swipe(380, 400, 200, 400); time.sleep(0.6)
    # 8. 弹窗:详情 / 重命名 / 移动
    # 详情弹窗(磁贴三点菜单)
    pg.locator(".tile .tile-more").first.tap(); time.sleep(0.4)
    pg.locator(".ctx-item", has_text="详情").tap(); time.sleep(0.5)
    pg.screenshot(path="/tmp/a08_detail.png")
    pg.locator(".modal .close").tap(); time.sleep(0.4)
    # 移动弹窗
    pg.locator("#tb-sort").tap(); time.sleep(0.3)
    pg.locator(".ctx-item", has_text="按名称").tap(); time.sleep(0.4)
    pg.locator(".tile .tile-more").first.tap(); time.sleep(0.4)
    pg.locator(".ctx-item", has_text="移动到").tap(); time.sleep(0.8)
    pg.screenshot(path="/tmp/a09_move.png")
    pg.locator(".modal .close").first.tap(); time.sleep(0.4)
    b.close()
print("DONE")
