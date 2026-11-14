#!/usr/bin/env python3
"""用户名重复渲染修复 + 抽屉宽度实测"""
import re, time
from playwright.sync_api import sync_playwright

BASE = "http://127.0.0.1:18098"

with sync_playwright() as p:
    b = p.chromium.launch()
    # PC
    pg = b.new_page(viewport={"width": 1280, "height": 800})
    pg.goto(BASE)
    pg.wait_for_selector(".login-card", timeout=8000)
    cap = pg.evaluate("fetch('/api/captcha').then(r => r.json())")
    code = "".join(re.findall(r">([^<]+)</text>", cap["svg"]))
    pg.evaluate(f"document.getElementById('cap-id').value = {cap['id']!r}")
    pg.fill("#in-user", "admin"); pg.fill("#in-pass", "admin123"); pg.fill("#in-captcha", code)
    pg.click("#btn-login"); pg.wait_for_selector("#files", timeout=8000); time.sleep(0.8)
    print("PC who 内用户名出现次数:", pg.evaluate("document.getElementById('whoBtn').innerText.trim().split('\\n').filter(s => s.includes('admin')).length"))
    pg.screenshot(path="/tmp/d1_pc.png")

    # 移动端抽屉宽度
    pg2 = b.new_page(viewport={"width": 390, "height": 844}, has_touch=True, is_mobile=True)
    login2 = pg2
    login2.goto(BASE)
    login2.wait_for_selector(".login-card", timeout=8000)
    cap = login2.evaluate("fetch('/api/captcha').then(r => r.json())")
    code = "".join(re.findall(r">([^<]+)</text>", cap["svg"]))
    login2.evaluate(f"document.getElementById('cap-id').value = {cap['id']!r}")
    login2.fill("#in-user", "admin"); login2.fill("#in-pass", "admin123"); login2.fill("#in-captcha", code)
    login2.click("#btn-login"); login2.wait_for_selector("#files", timeout=8000); time.sleep(0.8)
    login2.locator("#btn-menu").tap(); time.sleep(0.5)
    w = login2.evaluate("document.getElementById('sidebar').getBoundingClientRect().width")
    print("移动端抽屉宽度:", round(w), "px / 屏宽 390px")
    print("移动端 who 用户名次数:", login2.evaluate("document.getElementById('whoBtn').innerText.trim().split('\\n').filter(s => s.includes('admin')).length"))
    login2.screenshot(path="/tmp/d2_drawer.png")
    b.close()
print("DONE")
