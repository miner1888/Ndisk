#!/usr/bin/env python3
"""NDisk(二次重建版)—— BT / 磁力下载端到端验证(回环、确定性)。

不依赖公网:本地用 mktorrent 造种、transmission-cli 做种、起一个极简 HTTP tracker,
再让服务的 librqbit 引擎通过磁力完成「解析文件清单 → 下载 → 移入网盘」。

前置:mktorrent、transmission-cli(缺失则跳过)。
用法:python3 tests/bt_local_smoke.py [二进制路径]
"""
import hashlib
import http.client
import http.server
import json
import os
import re
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time
import urllib.parse

BIN = sys.argv[1] if len(sys.argv) > 1 else os.environ.get(
    "NDISK_BIN", "./target/debug/ndisk"
)
if os.name == "nt" and os.path.exists(BIN + ".exe"):
    BIN += ".exe"
PASS = 0
FAIL = 0
SEED_PORT = 0  # 由 main 填充


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


def bdecode(data, i=0):
    c = data[i:i + 1]
    if c == b"i":
        j = data.index(b"e", i)
        return int(data[i + 1:j]), j + 1
    if c == b"l":
        i += 1
        out = []
        while data[i:i + 1] != b"e":
            v, i = bdecode(data, i)
            out.append(v)
        return out, i + 1
    if c == b"d":
        i += 1
        out = {}
        while data[i:i + 1] != b"e":
            k, i = bdecode(data, i)
            v, i = bdecode(data, i)
            out[k] = v
        return out, i + 1
    j = data.index(b":", i)
    n = int(data[i:j])
    return data[j + 1:j + 1 + n], j + 1 + n


def info_slice(data):
    i = 1
    while data[i:i + 1] != b"e":
        k, i = bdecode(data, i)
        start = i
        _, i = bdecode(data, i)
        if k == b"info":
            return data[start:i]
    raise ValueError("no info")


class Tracker(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        pass

    def do_GET(self):
        peer = bytes([127, 0, 0, 1]) + struct.pack(">H", SEED_PORT)
        body = (b"d8:completei1e10:incompletei0e8:intervali30e5:peers6:" + peer + b"e")
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


class ThreadingServer(http.server.socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True


def call(port, method, path, body=None, token=""):
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
    global SEED_PORT
    if not os.path.exists(BIN):
        print(f"找不到二进制: {BIN}")
        return 2
    for tool in ("mktorrent", "transmission-cli"):
        if not shutil.which(tool):
            print(f"缺少 {tool},跳过 BT 测试")
            return 0

    work = tempfile.mkdtemp(prefix="ndisk-bt-")
    data_dir = os.path.join(work, "data")
    seed_dir = os.path.join(work, "seed")
    os.makedirs(seed_dir)
    payload = os.urandom(2 * 1024 * 1024 + 12345)
    file_path = os.path.join(seed_dir, "payload.bin")
    with open(file_path, "wb") as f:
        f.write(payload)

    SEED_PORT = free_port()
    tracker = ThreadingServer(("127.0.0.1", 0), Tracker)
    tracker_port = tracker.server_address[1]
    threading.Thread(target=tracker.serve_forever, daemon=True).start()
    announce = f"http://127.0.0.1:{tracker_port}/announce"

    torrent = os.path.join(work, "test.torrent")
    subprocess.run(["mktorrent", "-a", announce, "-o", torrent, "-l", "16", file_path],
                   check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    with open(torrent, "rb") as f:
        raw = f.read()
    infohash = hashlib.sha1(info_slice(raw)).hexdigest()
    magnet = "magnet:?xt=urn:btih:%s&dn=payload.bin&tr=%s" % (
        infohash, urllib.parse.quote(announce, safe=""))

    env = dict(os.environ, HOME=work)
    seeder = subprocess.Popen(
        ["transmission-cli", "-w", seed_dir, "-p", str(SEED_PORT), torrent],
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, env=env)
    time.sleep(3)  # 让做种端先起来并向 tracker 报到

    api_port = free_port()
    server = subprocess.Popen([BIN, "-p", str(api_port), "-d", data_dir],
                              stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    try:
        if not wait_ready(api_port, server):
            print("服务启动失败")
            print(server.stdout.read().decode(errors="replace"))
            return 2
        st, cap = call(api_port, "GET", "/api/captcha")
        code = captcha_answer(cap["svg"])
        st, r = call(api_port, "POST", "/api/auth/login",
                     {"username": "admin", "password": "admin123",
                      "captcha_id": cap["id"], "captcha": code})
        check(st == 200 and r.get("token"), "登录成功")
        token = r["token"]
        st, shares = call(api_port, "GET", "/api/shares", token=token)
        sid = next(s for s in shares["shares"] if s.get("home"))["id"]

        print("\n== BT 做种开关(默认关闭) ==")
        st, adm = call(api_port, "GET", "/api/admin/status", token=token)
        check(st == 200 and adm.get("bt_seed") is False, "默认:BT 不做种/不上传", f"{adm.get('bt_seed')}")
        st, _ = call(api_port, "POST", "/api/admin/offline-settings", {"bt_seed": True}, token=token)
        check(st == 200, "可开启 BT 做种")
        st, adm = call(api_port, "GET", "/api/admin/status", token=token)
        check(adm.get("bt_seed") is True, "设置已保存")
        st, _ = call(api_port, "POST", "/api/admin/offline-settings", {"bt_seed": False}, token=token)

        print(f"\n== 磁力下载(infohash={infohash}) ==")
        st, ad = call(api_port, "POST", "/api/offline/add",
                      {"dir": "下载", "url": magnet}, token=token)
        check(st == 200 and ad.get("kind") == "bittorrent", "添加磁力任务", f"{ad}")
        tid = ad.get("id")

        states = []
        done = False
        deadline = time.time() + 150
        last = {}
        while time.time() < deadline:
            st, lst = call(api_port, "GET", "/api/offline/list", token=token)
            t = next((x for x in lst.get("tasks", []) if x["id"] == tid), None)
            if t:
                last = t
                if not states or states[-1] != t.get("state"):
                    states.append(t.get("state"))
                if t.get("state") == "completed":
                    done = True
                    break
                if t.get("state") == "failed":
                    break
            time.sleep(1)
        print("    状态流转:", " → ".join(str(s) for s in states))
        check("downloading" in states or "completed" in states, "引擎进入下载流程", f"{states}")
        check(done, "磁力下载 completed", f"{last}")

        print("\n== 落盘与内容校验 ==")
        st, b = call(api_port, "GET", f"/api/browse/{sid}?path=%E4%B8%8B%E8%BD%BD", token=token)
        names = [e["name"] for e in b.get("entries", [])]
        check("payload.bin" in names, "文件出现在 下载 目录", f"{names}")
        st, body = call(api_port, "GET",
                        f"/api/media/{sid}?path=%E4%B8%8B%E8%BD%BD/payload.bin&dl=1", token=token)
        check(st == 200 and body == payload, "下载内容与源逐字节一致")
    finally:
        for p in (server, seeder):
            try:
                p.terminate()
                p.wait(timeout=6)
            except Exception:
                p.kill()
        tracker.shutdown()
        shutil.rmtree(work, ignore_errors=True)

    print(f"\n==== 通过 {PASS} / 失败 {FAIL} ====")
    return 0 if FAIL == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
