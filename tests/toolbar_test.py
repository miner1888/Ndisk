#!/usr/bin/env python3
"""移动端顶栏一行布局 + FAB 精简实测"""
import re, time
from playwright.sync_api import sync_playwright

BASE = "http://127.0.0.1:18098"

with sync_playwright() as p:
    b = p.chromium.launch()
    pg = b.new_page(viewport={"width": 390, "height": 844}, has_touch=True, is_mobile=True)
    pg.goto(BASE)
    errs = []
    pg.on("pageerror", lambda e: errs.append(str(e)))
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

    ids = ["btn-menu", "tb-view", "tb-sort", "tb-perm", "btn-search"]
    vis = {i: pg.locator(f"#{i}").is_visible() for i in ids}
    print("顶栏按钮可见:", vis)
    ys = {i: pg.locator(f"#{i}").bounding_box()["y"] for i in ids if vis[i]}
    print("同一行(y 相近):", ys)
    print("旧按钮隐藏: view-switch=", not pg.locator(".view-switch").is_visible(),
          "sort-select=", not pg.locator("#sort-by").is_visible(),
          "btn-upload=", not pg.locator("#btn-upload").is_visible())
    pg.screenshot(path="/tmp/t1_toolbar.png")

    # FAB 只剩两项
    pg.locator("#fabBtn").tap(); time.sleep(0.4)
    print("FAB 菜单项:", pg.locator(".fab-item").all_inner_texts())
    pg.locator("#fabBtn").tap(); time.sleep(0.3)

    # 排序菜单
    pg.locator("#tb-sort").tap(); time.sleep(0.4)
    menu_items = pg.locator(".ctx-menu .ctx-item").all_inner_texts()
    print("排序菜单:", menu_items)
    print("onclick 绑定:", pg.evaluate("typeof document.getElementById('tb-sort').onclick"))
    print("按钮可见性:", pg.evaluate("""(() => {
      const b = document.getElementById('tb-sort');
      const r = b.getBoundingClientRect();
      return JSON.stringify({x: Math.round(r.x), y: Math.round(r.y), w: r.width, h: r.height});
    })()"""))
    pg.evaluate("document.getElementById('tb-sort').dispatchEvent(new MouseEvent('click', {bubbles: true, clientX: 300, clientY: 20}))")
    time.sleep(0.4)
    print("手动 click 后菜单项:", pg.locator(".ctx-menu .ctx-item").all_inner_texts())
    print("JS错误:", errs)
    pg.locator(".ctx-menu .ctx-item", has_text="按大小").tap(); time.sleep(0.4)
    print("排序已切换:", pg.evaluate("localStorage.getItem('nd_sort')"))

    # 视图切换
    pg.locator("#tb-view").tap(); time.sleep(0.5)
    print("切换后列表视图存在:", pg.locator(".files-table").count() > 0)
    pg.locator("#tb-view").tap(); time.sleep(0.5)
    print("切回网格视图:", pg.locator(".grid").count() > 0)
    b.close()
print("DONE")
