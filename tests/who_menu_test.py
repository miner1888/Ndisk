#!/usr/bin/env python3
"""头像弹出账号菜单实测(PC + 移动端)"""
import re, time
from playwright.sync_api import sync_playwright

BASE = "http://127.0.0.1:18098"

def login(pg):
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
    time.sleep(0.8)

with sync_playwright() as p:
    b = p.chromium.launch()

    # PC 端
    pg = b.new_page(viewport={"width": 1280, "height": 800})
    login(pg)
    print("PC 旧按钮已移除:", pg.locator("#btn-admin").count() == 0 and pg.locator("#btn-logout").count() == 0)
    pg.locator("#whoBtn").click()
    time.sleep(0.4)
    items = pg.locator(".ctx-menu .ctx-item").all_inner_texts()
    print("PC 头像菜单:", items)
    pg.screenshot(path="/tmp/w1_pc_menu.png")
    pg.locator(".ctx-menu .ctx-item", has_text="退出登录").click()
    time.sleep(0.8)
    print("PC 退出后回登录页:", pg.locator(".login-card").count() > 0)

    # 移动端(抽屉内)
    pg2 = b.new_page(viewport={"width": 390, "height": 844}, has_touch=True, is_mobile=True)
    login(pg2)
    pg2.locator("#btn-menu").tap(); time.sleep(0.4)
    pg2.locator("#whoBtn").tap(); time.sleep(0.4)
    items2 = pg2.locator(".ctx-menu .ctx-item").all_inner_texts()
    print("移动端头像菜单:", items2)
    pg2.screenshot(path="/tmp/w2_mobile_menu.png")
    pg2.locator(".ctx-menu .ctx-item", has_text="管理后台").tap()
    time.sleep(0.8)
    print("移动端进管理后台:", pg2.locator(".admin-top").count() > 0)
    b.close()
print("DONE")
