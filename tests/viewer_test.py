#!/usr/bin/env python3
"""相册查看器实测:满屏 / 无刷新按钮 / 右缘左滑返回 / 中部滑动切图"""
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
    pg.fill("#in-user", "admin")
    pg.fill("#in-pass", "admin123")
    pg.fill("#in-captcha", code)
    pg.click("#btn-login")
    pg.wait_for_selector("#files", timeout=8000)
    time.sleep(1.0)

    cdp = pg.context.new_cdp_session(pg)
    def swipe(x1, y1, x2, y2, steps=8):
        cdp.send("Input.dispatchTouchEvent", {"type": "touchStart", "touchPoints": [{"x": x1, "y": y1}]})
        for i in range(1, steps + 1):
            cdp.send("Input.dispatchTouchEvent", {"type": "touchMove",
              "touchPoints": [{"x": x1 + (x2 - x1) * i / steps, "y": y1 + (y2 - y1) * i / steps}]})
        cdp.send("Input.dispatchTouchEvent", {"type": "touchEnd", "touchPoints": []})

    # 相册:无刷新按钮 + 有返回
    pg.locator("#btn-menu").tap(); time.sleep(0.4)
    pg.locator(".share-item[data-album]").tap()
    time.sleep(1.2)
    print("相册刷新按钮已移除:", pg.locator("#btn-album-refresh").count() == 0,
          "| 返回按钮:", pg.locator("#btn-album-back").is_visible())

    # 打开第一张图
    pg.locator(".photo").first.tap()
    time.sleep(1.0)
    viewer = pg.locator(".imgviewer")
    print("查看器打开:", viewer.count() == 1)
    st = pg.evaluate("""(() => {
      const s = document.querySelector('.iv-stage');
      const cs = getComputedStyle(s);
      return { pad: cs.padding, nav: getComputedStyle(document.querySelector('.iv-nav')).display };
    })()""")
    print("满屏 stage(0 内边距):", st["pad"], "| 箭头隐藏:", st["nav"] == "none")
    pg.screenshot(path="/tmp/v1_fullscreen.png")

    # 右缘向左滑 = 关闭查看器
    swipe(378, 400, 180, 400)
    time.sleep(0.6)
    print("右缘左滑后查看器已关闭:", pg.locator(".imgviewer").count() == 0)

    # 重新打开,中部左滑 = 下一张(回归)
    pg.locator(".photo").first.tap()
    time.sleep(0.8)
    c0 = pg.locator(".iv-count").inner_text()
    swipe(280, 400, 80, 400)
    time.sleep(0.6)
    c1 = pg.locator(".iv-count").inner_text()
    print(f"中部左滑切图(回归): {c0} -> {c1}")

    # Esc 关闭,回相册
    pg.keyboard.press("Escape")
    b.close()
print("DONE")
