#!/usr/bin/env python3
"""移动端创建用户卡片:复选框不再被拉伸"""
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

    pg.locator("#btn-menu").tap(); time.sleep(0.4)
    pg.locator("#whoBtn").tap(); time.sleep(0.4)
    pg.locator(".ctx-menu .ctx-item", has_text="管理后台").tap(); time.sleep(1.0)
    box = pg.locator("#nu-admin").bounding_box()
    print("复选框尺寸:", {k: round(box[k]) for k in ("x", "y", "width", "height")})
    print("复选框未被拉伸:", box["width"] < 40)
    lbl = pg.locator("label.check", has_text="管理员").bounding_box()
    print("标签尺寸:", {k: round(lbl[k]) for k in ("width", "height")})
    pg.screenshot(path="/tmp/u1_users.png")
    b.close()
print("DONE")
