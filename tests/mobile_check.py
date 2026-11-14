#!/usr/bin/env python3
"""移动端 UI 实测:登录 → 文件页(FAB) → 相册(返回) → 展开菜单 → 截图"""
import re, sys, time
from playwright.sync_api import sync_playwright

BASE = "http://127.0.0.1:18098"

with sync_playwright() as p:
    b = p.chromium.launch()
    pg = b.new_page(viewport={"width": 390, "height": 844}, has_touch=True, is_mobile=True)
    pg.goto(BASE)
    # 等登录卡片出现
    pg.wait_for_selector(".login-card", timeout=8000)
    # 自己取一份验证码(id + svg),填入隐藏域后登录
    cap = pg.evaluate("fetch('/api/captcha').then(r => r.json())")
    code = "".join(re.findall(r">([^<]+)</text>", cap["svg"]))
    pg.evaluate(f"document.getElementById('cap-id').value = {cap['id']!r}")
    pg.fill("#in-user", "admin")
    pg.fill("#in-pass", "admin123")
    pg.fill("#in-captcha", code)
    pg.click("#btn-login")
    pg.wait_for_selector("#files", timeout=8000)
    time.sleep(1.2)  # 等文件列表与 FAB 渲染

    fab = pg.locator("#fabWrap")
    print("FAB visible:", fab.is_visible(), "| fabBtn text:", pg.locator("#fabBtn").inner_text()[:10])
    print("toolbar buttons visible: upload=", pg.locator("#btn-upload").is_visible(),
          "sort=", pg.locator("#sort-by").is_visible(),
          "search-icon=", pg.locator("#btn-search").is_visible())
    pg.screenshot(path="/tmp/m1_files.png")

    # 展开菜单
    pg.locator("#fabBtn").tap()
    time.sleep(0.5)
    pg.screenshot(path="/tmp/m2_fab_open.png")
    items = pg.locator(".fab-item").all_inner_texts()
    print("FAB 菜单项:", items)
    pg.locator("#fabBtn").tap(); time.sleep(0.3)

    # 搜索态:点放大镜,输入框展开
    pg.locator("#btn-search").tap(); time.sleep(0.4)
    print("搜索输入框 visible:", pg.locator("#Fsearch").is_visible())
    pg.screenshot(path="/tmp/m5_search.png")
    pg.locator("#btn-scancel").tap(); time.sleep(0.4)
    print("取消后回正常工具栏:", pg.locator("#btn-search").is_visible())

    # 进入相册:先打开侧栏抽屉
    pg.locator("#btn-menu").tap(); time.sleep(0.5)
    pg.locator(".share-item[data-album]").tap()
    time.sleep(1.2)
    back = pg.locator("#btn-album-back")
    print("相册返回按钮 visible:", back.is_visible())
    print("相册页 FAB hidden:", not fab.is_visible())
    pg.screenshot(path="/tmp/m3_album.png")

    back.tap()
    time.sleep(1.0)
    print("返回后回到文件页:", pg.locator("#fabWrap").is_visible(), "albumView gone:", pg.locator(".album-grid").count() == 0)
    pg.screenshot(path="/tmp/m4_back.png")
    b.close()
print("DONE")
