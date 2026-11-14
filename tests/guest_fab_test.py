#!/usr/bin/env python3
"""游客模式 FAB 隐藏实测"""
import re, time, os
from playwright.sync_api import sync_playwright

BASE = "http://127.0.0.1:18098"
PUB = "/tmp/ndisk-ui-test/public"

os.makedirs(PUB, exist_ok=True)
with open(PUB + "/readme.txt", "w") as f:
    f.write("public file for guest")

with sync_playwright() as p:
    b = p.chromium.launch()

    # 1. 管理员创建游客可访问的共享目录
    pg = b.new_page(viewport={"width": 390, "height": 844}, has_touch=True, is_mobile=True)
    pg.goto(BASE)
    pg.wait_for_selector("#files", timeout=8000)
    pg.locator("#btn-menu").tap(); time.sleep(0.4)
    pg.locator("#whoBtn").tap(); time.sleep(0.4)
    pg.locator(".ctx-menu .ctx-item", has_text="登录").tap()
    pg.wait_for_selector(".login-card", timeout=8000)
    cap = pg.evaluate("fetch('/api/captcha').then(r => r.json())")
    code = "".join(re.findall(r">([^<]+)</text>", cap["svg"]))
    pg.evaluate(f"document.getElementById('cap-id').value = {cap['id']!r}")
    pg.fill("#in-user", "admin"); pg.fill("#in-pass", "admin123"); pg.fill("#in-captcha", code)
    pg.click("#btn-login"); pg.wait_for_selector("#files", timeout=8000); time.sleep(0.6)
    r = pg.evaluate("fetch('/api/shares', {method:'POST', headers:{'Content-Type':'application/json'}, body: JSON.stringify({name:'公共区', host_path: arguments_host})}).then(r=>r.json())".replace("arguments_host", repr(PUB)))
    print("创建共享:", r)
    sid = r["id"]
    r2 = pg.evaluate(f"fetch('/api/shares/{sid}/rules', {{method:'POST', headers:{{'Content-Type':'application/json'}}, body: JSON.stringify({{rel_path:'', access:'guest'}})}}).then(r=>r.json())")
    print("游客规则:", r2)
    pg.evaluate("localStorage.clear()")
    pg.context.clear_cookies()

    # 2. 游客访问(有可浏览空间时直接进入游客模式主界面)
    pg.goto(BASE)
    pg.wait_for_selector("#files", timeout=8000)
    time.sleep(1.0)
    print("游客直接进入主界面:", pg.locator("#files").count() > 0)
    print("游客已进入文件页:", pg.locator("#files").count() > 0)
    # 游客模式下 bootApp 自动选中可浏览空间,无需手动切换
    fab_hidden = pg.evaluate("document.getElementById('fabWrap').classList.contains('hidden')")
    print("游客 FAB 隐藏:", fab_hidden)
    print("游客菜单无菜单项:", pg.locator(".fab-item").count() == 0)
    pg.screenshot(path="/tmp/g1_guest.png")
    b.close()
print("DONE")
