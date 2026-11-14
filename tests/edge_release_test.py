#!/usr/bin/env python3
"""右缘返回:划动未松手不返回,松手后才返回"""
import re, time
from playwright.sync_api import sync_playwright

BASE = "http://127.0.0.1:18098"
with sync_playwright() as p:
    b = p.chromium.launch()
    pg = b.new_page(viewport={"width": 390, "height": 844}, has_touch=True, is_mobile=True)
    pg.goto(BASE)
    pg.wait_for_selector(".login-card", timeout=8000)
    cap = pg.evaluate("fetch('/api/captcha').then(r => r.json())")
    code = "".join(re.findall(r">([^<]+)</text>", cap["svg"]))
    pg.evaluate(f"document.getElementById('cap-id').value = {cap['id']!r}")
    pg.fill("#in-user", "admin"); pg.fill("#in-pass", "admin123"); pg.fill("#in-captcha", code)
    pg.click("#btn-login"); pg.wait_for_selector("#files", timeout=8000); time.sleep(0.8)

    cdp = pg.context.new_cdp_session(pg)
    pg.locator("#btn-menu").tap(); time.sleep(0.4)
    pg.locator("#whoBtn").tap(); time.sleep(0.4)
    pg.locator(".ctx-menu .ctx-item", has_text="管理后台").tap(); time.sleep(1.0)
    print("已进入后台:", pg.locator(".admin-wrap").count() == 1)

    # 划到一半不松手
    cdp.send("Input.dispatchTouchEvent", {"type": "touchStart", "touchPoints": [{"x": 380, "y": 400}]})
    for i in range(1, 9):
        cdp.send("Input.dispatchTouchEvent", {"type": "touchMove", "touchPoints": [{"x": 380 - i * 25, "y": 400}]})
    time.sleep(0.6)
    print("划满 200px 未松手,仍在后台:", pg.locator(".admin-wrap").count() == 1)
    # 松手
    cdp.send("Input.dispatchTouchEvent", {"type": "touchEnd", "touchPoints": []})
    time.sleep(0.8)
    print("松手后返回文件页:", pg.locator(".admin-wrap").count() == 0)
    b.close()
print("DONE")
