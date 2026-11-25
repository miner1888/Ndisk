#!/usr/bin/env python3
"""NDisk(二次重建版)—— 媒体能力(ffmpeg)端到端验证。

前置:系统 PATH 里存在 ffmpeg。脚本用 ffmpeg 现场生成测试视频/图片,验证:
  - 非浏览器原生视频(avi)→ /api/media/status 触达转码 → 轮询到 ready
  - 转码产物 /api/media?t=1 返回可用的 mp4(ftyp)
  - 视频封面 /api/media/thumb 返回 JPEG
  - 图片缩略图 /api/media/imgthumb 返回 JPEG
  - 开启"自动转码替换"后,新上传的 avi 在后台被替换为 mp4
用法:python3 tests/media_smoke.py [二进制路径]
"""
import http.client
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import time

BIN = sys.argv[1] if len(sys.argv) > 1 else os.environ.get(
    "NDISK_BIN", "./target/debug/ndisk"
)
if os.name == "nt" and os.path.exists(BIN + ".exe"):
    BIN += ".exe"
PASS = 0
FAIL = 0


def check(cond, label, extra=""):
    global PASS, FAIL
    if cond:
        PASS += 1
        print(f"  \033[32mPASS\033[0m {label}")
    else:
        FAIL += 1
        print(f"  \033[31mFAIL\033[0m {label} {extra}")


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    p = s.getsockname()[1]
    s.close()
    return p


def call(port, method, path, body=None, token="", raw=False):
    h = {}
    if token:
        h["Authorization"] = "Bearer " + token
    data = None
    if body is not None:
        if isinstance(body, (bytes, bytearray)):
            data = bytes(body)
        else:
            data = json.dumps(body).encode()
            h["Content-Type"] = "application/json"
    c = http.client.HTTPConnection("127.0.0.1", port, timeout=30)
    c.request(method, path, body=data, headers=h)
    r = c.getresponse()
    pl = r.read()
    ct = r.getheader("Content-Type", "") or ""
    c.close()
    if raw:
        return r.status, pl, ct
    if "json" in ct:
        try:
            return r.status, json.loads(pl)
        except Exception:
            return r.status, None
    return r.status, pl


def wait_ready(port, proc, timeout=30):
    deadline = time.time() + timeout
    while time.time() < deadline:
        if proc.poll() is not None:
            return False
        try:
            socket.create_connection(("127.0.0.1", port), 1).close()
            return True
        except OSError:
            time.sleep(0.2)
    return False


def captcha_answer(svg):
    return "".join(re.findall(r"<text[^>]*>([^<])</text>", svg))


def gen_media(work):
    avi = os.path.join(work, "clip.avi")
    png = os.path.join(work, "pic.png")
    mp4 = os.path.join(work, "native.mp4")
    subprocess.run(
        ["ffmpeg", "-hide_banner", "-loglevel", "error", "-y",
         "-f", "lavfi", "-i", "testsrc=size=160x120:rate=10", "-t", "2",
         "-c:v", "mpeg4", "-q:v", "6", avi],
        check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    subprocess.run(
        ["ffmpeg", "-hide_banner", "-loglevel", "error", "-y",
         "-f", "lavfi", "-i", "testsrc=size=160x120:rate=10", "-t", "2",
         "-c:v", "libx264", "-pix_fmt", "yuv420p", mp4],
        check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    subprocess.run(
        ["ffmpeg", "-hide_banner", "-loglevel", "error", "-y",
         "-f", "lavfi", "-i", "testsrc=size=400x300", "-frames:v", "1", png],
        check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    return avi, png, mp4


def main():
    if not os.path.exists(BIN):
        print(f"找不到二进制: {BIN}")
        return 2
    if not shutil.which("ffmpeg"):
        print("本机无 ffmpeg,跳过媒体测试")
        return 0
    work = tempfile.mkdtemp(prefix="ndisk-media-")
    data_dir = os.path.join(work, "data")
    avi, png, mp4 = gen_media(work)
    port = free_port()
    proc = subprocess.Popen([BIN, "-p", str(port), "-d", data_dir],
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    try:
        if not wait_ready(port, proc):
            print("启动失败")
            print(proc.stdout.read().decode(errors="replace"))
            return 2
        st, cap = call(port, "GET", "/api/captcha")
        code = captcha_answer(cap["svg"])
        st, r = call(port, "POST", "/api/auth/login",
                     {"username": "admin", "password": "admin123",
                      "captcha_id": cap["id"], "captcha": code})
        check(st == 200 and r.get("token"), "登录成功")
        token = r["token"]
        st, shares = call(port, "GET", "/api/shares", token=token)
        sid = next(s for s in shares["shares"] if s.get("home"))["id"]

        print("\n== 视频转码(按需) ==")
        with open(avi, "rb") as f:
            st, up = call(port, "POST",
                          f"/api/upload/direct?share_id={sid}&path=&filename=clip.avi",
                          body=f.read(), token=token)
        check(st == 200, "上传 avi")
        st, status = call(port, "GET", f"/api/media/status/{sid}?path=clip.avi", token=token)
        check(st == 200 and status.get("state") in ("converting", "ready"),
              "avi 触发转码(converting)", f"{status}")

        ready = False
        deadline = time.time() + 90
        while time.time() < deadline:
            st, status = call(port, "GET", f"/api/media/status/{sid}?path=clip.avi", token=token)
            if status.get("state") == "ready":
                ready = True
                break
            if status.get("state") not in ("converting",):
                break
            time.sleep(1.5)
        check(ready, "转码完成(state=ready)", f"{status}")

        st, body, ct = call(port, "GET", f"/api/media/{sid}?path=clip.avi&t=1",
                            token=token, raw=True)
        check(st == 200 and len(body) > 100 and body[4:8] == b"ftyp",
              "转码产物为可用 mp4(ftyp)", f"status={st} ct={ct} len={len(body)}")

        st, jpg, ct = call(port, "GET", f"/api/media/thumb/{sid}?path=clip.avi",
                           token=token, raw=True)
        check(st == 200 and jpg[:2] == b"\xff\xd8", "视频封面为 JPEG", f"status={st} ct={ct}")

        print("\n== 图片缩略图 ==")
        with open(png, "rb") as f:
            st, _ = call(port, "POST",
                         f"/api/upload/direct?share_id={sid}&path=&filename=pic.png",
                         body=f.read(), token=token)
        check(st == 200, "上传图片")
        st, jpg2, ct = call(port, "GET", f"/api/media/imgthumb/{sid}?path=pic.png",
                            token=token, raw=True)
        check(st == 200 and jpg2[:2] == b"\xff\xd8", "图片缩略图为 JPEG", f"status={st} ct={ct}")

        print("\n== 原生 MP4 直接播放(不转码) ==")
        with open(mp4, "rb") as f:
            st, _ = call(port, "POST",
                         f"/api/upload/direct?share_id={sid}&path=&filename=native.mp4",
                         body=f.read(), token=token)
        check(st == 200, "上传 native.mp4")
        st, status = call(port, "GET", f"/api/media/status/{sid}?path=native.mp4", token=token)
        check(st == 200 and status.get("direct") is True and status.get("kind") == "video",
              "原生 mp4 走直连播放(direct=true,不转码)", f"{status}")
        st, mp4body, ct = call(port, "GET", f"/api/media/{sid}?path=native.mp4", token=token, raw=True)
        check(st == 200 and mp4body[4:8] == b"ftyp" and ct == "video/mp4",
              "直连返回原始 mp4")

        print("\n== 自动转码替换(后台开关) ==")
        st, _ = call(port, "POST", "/api/admin/transcode-replace", {"enabled": True}, token=token)
        check(st == 200, "开启自动转码替换")
        with open(avi, "rb") as f:
            st, _ = call(port, "POST",
                         f"/api/upload/direct?share_id={sid}&path=&filename=auto.avi",
                         body=f.read(), token=token)
        check(st == 200, "上传 auto.avi")
        replaced = False
        deadline = time.time() + 90
        while time.time() < deadline:
            try:
                st, body, _ = call(port, "GET", f"/api/media/{sid}?path=auto.avi&dl=1",
                                   token=token, raw=True)
                if len(body) > 8 and body[4:8] == b"ftyp":
                    replaced = True
                    break
            except Exception:
                pass
            time.sleep(2)
        check(replaced, "auto.avi 内容被后台替换为 mp4(文件名保持不变)")
        st, _ = call(port, "POST", "/api/admin/transcode-replace", {"enabled": False}, token=token)

        print("\n== 缓存统计/清理 ==")
        st, adm = call(port, "GET", "/api/admin/status", token=token)
        check(st == 200 and adm.get("ffmpeg"), "后台识别到 ffmpeg")
        check(adm.get("cache", {}).get("transcodes", 0) >= 1, "缓存登记了转码产物",
              f"{adm.get('cache')}")
        st, cl = call(port, "POST", "/api/admin/cache/clear", {}, token=token)
        check(st == 200 and cl.get("cache", {}).get("bytes", 1) == 0, "一键清理缓存后占用归零",
              f"{cl.get('cache')}")
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=8)
        except Exception:
            proc.kill()
        shutil.rmtree(work, ignore_errors=True)

    print(f"\n==== 通过 {PASS} / 失败 {FAIL} ====")
    return 0 if FAIL == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
