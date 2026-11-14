#!/usr/bin/env python3
"""FAB 展开视觉 + 下拉刷新实测"""
import re, time
from playwright.sync_api import sync_playwright

BASE = "http://127.0.0.1:18098"

with sync_playwright() as p:
    b = p.chromium.launch()
    pg = b.new_page(viewport={"width": 390, "height": 844}, has_touch=True, is_mobile=True)
    pg.goto(BASE)
    errs = []
    pg.on("pageerror", lambda e: errs.append(str(e)))
    logs = []
    pg.on("console", lambda m: logs.append(m.type + ":" + m.text))
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

    # FAB 菜单项(应无"刷新")
    pg.locator("#fabBtn").tap()
    time.sleep(0.4)
    items = pg.locator(".fab-item").all_inner_texts()
    print("FAB 菜单项:", items)
    print("刷新已移除:", all("刷新" not in x for x in items))
    pg.screenshot(path="/tmp/f1_fab_open.png")
    pg.locator("#fabBtn").tap(); time.sleep(0.3)

    # 标记当前 DOM,刷新后应被重建
    pg.evaluate("const n = document.querySelector('#files > *'); if (n) n.setAttribute('data-mark','1')")
    marked = pg.evaluate("!!document.querySelector('#files [data-mark]')")
    print("标记已打:", marked)

    pg.evaluate("""window.__evts = [];
      ['touchstart','touchmove','touchend','touchcancel'].forEach(t =>
        document.addEventListener(t, e => window.__evts.push(t + '@' + (e.touches[0] ? Math.round(e.touches[0].clientY) : 'end')), true));""")
    pg.evaluate("""window.__probe = [];
      document.addEventListener('touchstart', e => {
        const el = e.target;
        window.__probe.push('target=' + (el.id || el.className || el.tagName).toString().slice(0, 30)
          + ' scrollTop=' + document.getElementById('files').scrollTop
          + ' closestHit=' + !!el.closest('#sidebar, #sideMask, .modal-mask, .selbar, .upload-panel, #fabWrap, .ctx-menu, .audiobar, .toolbar, button, a, input, label'));
      }, true);""")

    cdp = pg.context.new_cdp_session(pg)
    cdp.send("Input.dispatchTouchEvent", {"type": "touchStart", "touchPoints": [{"x": 200, "y": 200}]})
    for i in range(1, 9):
        cdp.send("Input.dispatchTouchEvent", {"type": "touchMove", "touchPoints": [{"x": 200, "y": 200 + i * 30}]})
    time.sleep(0.3)
    bar = pg.locator(".ptr")
    txt = pg.locator("#ptrText").inner_text() if bar.count() else "(无指示条)"
    vis = bar.is_visible() if bar.count() else False
    print("下拉中指示条 visible:", vis, "| 文案:", txt)
    print("完整控制台日志:", logs)
    print("指示条 in DOM:", pg.evaluate("!!document.querySelector('.ptr')"))
    pg.screenshot(path="/tmp/f2_pulling.png")
    cdp.send("Input.dispatchTouchEvent", {"type": "touchEnd", "touchPoints": []})
    print("触摸事件流:", pg.evaluate("window.__evts"))
    print("探针:", pg.evaluate("window.__probe"))
    print("JS错误:", errs[-3:])
    print("控制台:", logs[-5:])
    time.sleep(0.5)
    loading = pg.locator("#ptrText").inner_text() if pg.locator("#ptrBar").count() else ""
    print("松手后文案:", loading)
    time.sleep(1.2)
    gone = pg.evaluate("!document.querySelector('#files [data-mark]')")
    hidden = bar.count() == 0 or not bar.evaluate("el => el.classList.contains('show')")
    print("列表已重渲染(标记消失):", gone, "| 指示条已收起:", hidden)
    b.close()
print("DONE")
