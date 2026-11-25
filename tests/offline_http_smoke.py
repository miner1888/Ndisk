#!/usr/bin/env python3
"""NDisk(二次重建版)—— 离线下载(HTTP 直链)端到端验证。

本地起一个支持 Range 的 HTTP 服务器,提供一个 >8MB 文件,让服务的离线引擎
以 8 段并行方式下载,验证:
  - /api/offline/add 建任务、状态流转(parsing→downloading→completed)
  - 落盘到 个人空间/下载 且内容与源一致
  - /api/offline/events(SSE)可用
  - 非法/不支持链接被拒绝
  - 磁力任务可创建并在取消后进入 cancelled
  - .torrent 直链自动识别为 BT;种子文件上传建任务;非法种子内容被拒绝
用法:python3 tests/offline_http_smoke.py [二进制路径]
"""
import http.client
import http.server
import json
import os
import re
import shutil
import socket
import socketserver
import subprocess
import sys
import tempfile
import threading
import time

BIN = sys.argv[1] if len(sys.argv) > 1 else os.environ.get(
    "NDISK_BIN", "./target/debug/ndisk"
)
if os.name == "nt" and os.path.exists(BIN + ".exe"):
    BIN += ".exe"
PASS = 0
FAIL = 0
PAYLOAD = os.urandom(12 * 1024 * 1024)  # 12MB,触发多段并行
FILENAME = "sample.bin"


def benc(v):
    """最小 bencode 编码(仅覆盖本脚本构造种子的需要)。"""
    if isinstance(v, int):
        return b"i%de" % v
    if isinstance(v, str):
        v = v.encode()
    if isinstance(v, bytes):
        return str(len(v)).encode() + b":" + v
    if isinstance(v, list):
        return b"l" + b"".join(benc(x) for x in v) + b"e"
    if isinstance(v, dict):
        return b"d" + b"".join(benc(k) + benc(v[k]) for k in sorted(v)) + b"e"
    raise TypeError(type(v))


# 最小合法单文件种子(单文件任务会被引擎自动开始下载)。
def make_torrent(name):
    return benc({
        "announce": "http://127.0.0.1:1/announce",
        "info": {
            "name": name,
            "length": 1000,
            "piece length": 16384,
            "pieces": os.urandom(20),
        },
    })


# 上传用与直链用必须是不同 info-hash,避免同一会话内的任务竞态。
TORRENT = make_torrent("smoke-file.bin")
TORRENT_DL = make_torrent("smoke-dl.bin")


def multipart(fields):
    """构造 multipart/form-data 请求体。fields: [(name, str|(filename, bytes))]。"""
    boundary = "----ndisk-smoke-boundary"
    out = b""
    for name, value in fields:
        out += b"--" + boundary.encode() + b"\r\n"
        if isinstance(value, tuple):
            out += b'Content-Disposition: form-data; name="' + name.encode() + \
                   b'"; filename="' + value[0].encode() + b'"\r\n'
            out += b"Content-Type: application/x-bittorrent\r\n\r\n"
            out += value[1] + b"\r\n"
        else:
            out += b'Content-Disposition: form-data; name="' + name.encode() + b'"\r\n\r\n'
            out += value.encode() + b"\r\n"
    out += b"--" + boundary.encode() + b"--\r\n"
    return boundary, out


def call_multipart(port, method, path, boundary, body, token=""):
    h = {"Content-Type": f"multipart/form-data; boundary={boundary}"}
    if token:
        h["Authorization"] = "Bearer " + token
    c = http.client.HTTPConnection("127.0.0.1", port, timeout=30)
    c.request(method, path, body=body, headers=h)
    r = c.getresponse()
    pl = r.read()
    c.close()
    try:
        return r.status, json.loads(pl)
    except Exception:
        return r.status, None


def check(cond, label, extra=""):
    global PASS, FAIL
    if cond:
        PASS += 1
        print(f"  \033[32mPASS\033[0m {label}")
    else:
        FAIL += 1
        print(f"  \033[31mFAIL\033[0m {label} {extra}")


class RangeHandler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        pass

    def do_GET(self):
        if self.path.endswith("/real.torrent"):
            body = TORRENT_DL
            self.send_response(200)
            self.send_header("Content-Type", "application/x-bittorrent")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        total = len(PAYLOAD)
        rng = self.headers.get("Range")
        if rng and rng.startswith("bytes="):
            spec = rng[len("bytes="):].split(",")[0]
            s, _, e = spec.partition("-")
            start = int(s) if s else 0
            end = int(e) if e else total - 1
            end = min(end, total - 1)
            if start > end:
                self.send_response(416)
                self.send_header("Content-Range", f"bytes */{total}")
                self.send_header("Content-Length", "0")
                self.end_headers()
                return
            body = PAYLOAD[start:end + 1]
            self.send_response(206)
            self.send_header("Content-Type", "application/octet-stream")
            self.send_header("Accept-Ranges", "bytes")
            self.send_header("Content-Range", f"bytes {start}-{end}/{total}")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        else:
            self.send_response(200)
            self.send_header("Content-Type", "application/octet-stream")
            self.send_header("Accept-Ranges", "bytes")
            self.send_header("Content-Length", str(total))
            self.end_headers()
            self.wfile.write(PAYLOAD)


class ThreadingServer(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True


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


def main():
    if not os.path.exists(BIN):
        print(f"找不到二进制: {BIN}")
        return 2
    web = ThreadingServer(("127.0.0.1", 0), RangeHandler)
    web_port = web.server_address[1]
    threading.Thread(target=web.serve_forever, daemon=True).start()

    work = tempfile.mkdtemp(prefix="ndisk-offline-")
    data_dir = os.path.join(work, "data")
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

        print("\n== 参数校验 ==")
        st, r = call(port, "POST", "/api/offline/add", {"url": "ftp://x/y"}, token=token)
        check(st == 400, "不支持的协议被拒绝")
        st, r = call(port, "POST", "/api/offline/add", {"url": "not-a-link"}, token=token)
        check(st == 400, "无法识别的链接被拒绝")
        st, r = call(port, "POST", "/api/offline/add",
                     {"url": f"http://127.0.0.1:{web_port}/{FILENAME}.torrent"}, token=token)
        check(st == 200 and r.get("kind") == "bittorrent", ".torrent 直链自动识别为 BT", f"{r}")
        if r.get("id"):
            call(port, "POST", "/api/offline/cancel", {"id": r["id"]}, token=token)

        print("\n== HTTP 直链(多段并行) ==")
        url = f"http://127.0.0.1:{web_port}/{FILENAME}"
        st, ad = call(port, "POST", "/api/offline/add", {"dir": "下载", "url": url}, token=token)
        check(st == 200 and ad.get("kind") == "http", "添加 HTTP 离线任务", f"{ad}")
        tid = ad.get("id")

        done = False
        seen_downloading = False
        deadline = time.time() + 90
        last = {}
        while time.time() < deadline:
            st, lst = call(port, "GET", "/api/offline/list", token=token)
            task = next((t for t in lst.get("tasks", []) if t["id"] == tid), None)
            if task:
                last = task
                if task.get("state") == "downloading":
                    seen_downloading = True
                if task.get("state") == "completed":
                    done = True
                    break
                if task.get("state") == "failed":
                    break
            time.sleep(1)
        check(seen_downloading, "任务进入 downloading 状态")
        check(done, "任务 completed", f"{last}")
        if done:
            check(last.get("size") == len(PAYLOAD) and last.get("downloaded") == len(PAYLOAD),
                  "任务记录的大小与源一致")

        print("\n== 落盘到 个人空间/下载 ==")
        st, b = call(port, "GET", f"/api/browse/{sid}?path=%E4%B8%8B%E8%BD%BD", token=token)
        names = [e["name"] for e in b.get("entries", [])]
        check(FILENAME in names, "文件出现在 下载 目录", f"{names}")
        st, body, _ = call(port, "GET",
                           f"/api/media/{sid}?path=%E4%B8%8B%E8%BD%BD/{FILENAME}&dl=1",
                           token=token, raw=True)
        check(st == 200 and body == PAYLOAD, "下载内容与源逐字节一致")

        print("\n== SSE 事件流 ==")
        st, ct, first = None, None, b""
        try:
            c = http.client.HTTPConnection("127.0.0.1", port, timeout=8)
            c.request("GET", f"/api/offline/events?token={token}")
            resp = c.getresponse()
            st = resp.status
            ct = resp.getheader("Content-Type", "")
            first = resp.readline()
            c.close()
        except Exception as e:
            st = f"exc {e}"
        check(st == 200 and "text/event-stream" in (ct or ""), "SSE 端点返回 text/event-stream",
              f"status={st} ct={ct}")
        check(b"snapshot" in first, "收到首个 SSE 事件(snapshot)", f"{first!r}")

        print("\n== 磁力任务创建/取消 ==")
        st, ad2 = call(port, "POST", "/api/offline/add",
                       {"dir": "下载", "url": "magnet:?xt=urn:btih:" + "0" * 40}, token=token)
        check(st == 200 and ad2.get("kind") == "bittorrent", "添加磁力任务")
        tid2 = ad2.get("id")
        st, _ = call(port, "POST", "/api/offline/cancel", {"id": tid2}, token=token)
        check(st == 200, "取消磁力任务")
        time.sleep(1)
        st, lst = call(port, "GET", "/api/offline/list", token=token)
        t2 = next((t for t in lst.get("tasks", []) if t["id"] == tid2), None)
        check(t2 and t2.get("state") == "cancelled", "磁力任务进入 cancelled", f"{t2}")

        def wait_bt_active(tid, timeout=60):
            """等 BT 任务解析完成进入 downloading/awaiting_select;失败即返回。"""
            deadline = time.time() + timeout
            last = {}
            while time.time() < deadline:
                st, lst = call(port, "GET", "/api/offline/list", token=token)
                t = next((x for x in lst.get("tasks", []) if x["id"] == tid), None)
                if t:
                    last = t
                    if t.get("state") in ("downloading", "awaiting_select", "failed"):
                        break
                time.sleep(1)
            return last

        print("\n== 种子文件上传 ==")
        bnd, body = multipart([("dir", "下载"), ("file", ("smoke.torrent", TORRENT))])
        st, ad3 = call_multipart(port, "POST", "/api/offline/add-torrent", bnd, body, token=token)
        check(st == 200 and ad3.get("kind") == "bittorrent" and ad3.get("name") == "smoke-file.bin",
              "上传种子建任务(内容校验 + 名称提取)", f"{ad3}")
        t3 = wait_bt_active(ad3.get("id", -1))
        check(t3.get("state") in ("downloading", "awaiting_select"), "种子解析后进入可下载状态", f"{t3}")
        st, _ = call(port, "POST", "/api/offline/cancel", {"id": ad3.get("id")}, token=token)
        check(st == 200, "取消种子任务")

        bnd, body = multipart([("file", ("bad.torrent", b"this is not bencode"))])
        st, _ = call_multipart(port, "POST", "/api/offline/add-torrent", bnd, body, token=token)
        check(st == 400, "非法种子内容被拒绝")

        print("\n== .torrent 直链下载 ==")
        st, ad4 = call(port, "POST", "/api/offline/add",
                       {"dir": "下载", "url": f"http://127.0.0.1:{web_port}/real.torrent"}, token=token)
        check(st == 200 and ad4.get("kind") == "bittorrent", "直链种子识别为 BT 任务", f"{ad4}")
        t4 = wait_bt_active(ad4.get("id", -1))
        check(t4.get("state") in ("downloading", "awaiting_select"), "直链种子解析后进入可下载状态", f"{t4}")
        st, _ = call(port, "POST", "/api/offline/cancel", {"id": ad4.get("id")}, token=token)
        check(st == 200, "取消直链种子任务")
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=8)
        except Exception:
            proc.kill()
        web.shutdown()
        shutil.rmtree(work, ignore_errors=True)

    print(f"\n==== 通过 {PASS} / 失败 {FAIL} ====")
    return 0 if FAIL == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
