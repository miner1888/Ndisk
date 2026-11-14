#!/usr/bin/env python3
"""PC 端:点相册后侧栏应高亮相册,点空间后高亮切回"""
import re, time
from playwright.sync_api import sync_playwright

BASE = "http://127.0.0.1:18098"

with sync_playwright() as p:
    b = p.chromium.launch()
    pg = b.new_page(viewport={"width": 1280, "height": 800})
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

    def active():
        return pg.evaluate("""(() => {
          const a = document.querySelector('.share-item.active');
          return a ? (a.dataset.album ? '相册' : '空间:' + a.querySelector('span:nth-child(2)').textContent) : '无';
        })()""")

    print("初始高亮:", active())
    pg.locator(".share-item[data-album]").click()
    time.sleep(1.0)
    print("点击相册后高亮:", active(), "| 相册网格:", pg.locator(".album-grid").count() > 0)
    pg.screenshot(path="/tmp/sb1_album.png")
    pg.locator(".share-item[data-share]").first.click()
    time.sleep(1.0)
    print("点回空间后高亮:", active(), "| 相册网格已退出:", pg.locator(".album-grid").count() == 0)
    b.close()
print("DONE")
