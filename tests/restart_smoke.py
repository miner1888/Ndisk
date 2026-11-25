#!/usr/bin/env python3
"""NDisk(二次重建版)—— 离线下载"进程中断"行为验证(回环、确定性)。

本地起一个慢速 HTTP 文件服务器(64MB / ~1MB/s),让直链任务稳定停留在
downloading;再添加一个磁力任务停留 parsing;然后 **强杀服务进程**(模拟崩溃 /
断电,非优雅退出),重启后验证:
  - 所有中间态(downloading / parsing)任务必须被置为 failed,错误含"服务重启";
  - 不得有任何任务永远停留在"下载中"(前端会无限计数);
  - 失败任务可以被正常删除。

用法:python3 tests/restart_smoke.py [二进制路径]
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


def check(cond, label, extra=""):
    global PASS, FAIL
    if cond:
        PASS += 1
        print(f"  \033[32mPASS\033[0m {label}")
    else:
        FAIL += 1
        print(f"  \033[31mFAIL\033[0m {label} {extra}")


class StallHandler(http.server.BaseHTTPRequestHandler):
    """64MB 响应按 ~1MB/s 慢发,保证服务进程被杀时任务仍在 downloading。"""

    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        pass

    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("Content-Length", str(64 * 1024 * 1024))
        self.end_headers()
        try:
            for _ in range(256):
                time.sleep(0.25)
                self.wfile.write(b"x" * 262144)
        except Exception:
            pass


class ThreadingServer(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    p = s.getsockname()[1]
    s.close()
    return p


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
    c.close()
    if "json" in (r.getheader("Content-Type", "") or ""):
        try:
            return r.status, json.loads(pl)
        except Exception:
            return r.status, None
    return r.status, pl


def wait_ready(port, timeout=30):
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            socket.create_connection(("127.0.0.1", port), 1).close()
            return True
        except OSError:
            time.sleep(0.2)
    return False


def captcha_answer(svg):
    return "".join(re.findall(r"<text[^>]*>([^<])</text>", svg))


def boot(bin_path, data_dir, port):
    proc = subprocess.Popen([bin_path, "-p", str(port), "-d", data_dir],
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    ok = wait_ready(port)
    return proc, ok


def login(port):
    st, cap = call(port, "GET", "/api/captcha")
    code = captcha_answer(cap["svg"])
    st, r = call(port, "POST", "/api/auth/login",
                 {"username": "admin", "password": "admin123",
                  "captcha_id": cap["id"], "captcha": code})
    return r.get("token") if st == 200 else None


def task_by_id(port, token, tid):
    st, lst = call(port, "GET", "/api/offline/list", token=token)
    return next((t for t in lst.get("tasks", []) if t["id"] == tid), None)


def main():
    if not os.path.exists(BIN):
        print(f"找不到二进制: {BIN}")
        return 2
    stall = ThreadingServer(("127.0.0.1", 0), StallHandler)
    stall_port = stall.server_address[1]
    threading.Thread(target=stall.serve_forever, daemon=True).start()

    work = tempfile.mkdtemp(prefix="ndisk-restart-")
    data_dir = os.path.join(work, "data")
    port = free_port()
    proc, ok = boot(BIN, data_dir, port)
    try:
        check(ok, "服务首次启动")
        token = login(port)
        check(bool(token), "登录成功")

        print("\n== 制造两个中间态任务 ==")
        st, ad = call(port, "POST", "/api/offline/add",
                      {"dir": "下载", "url": f"http://127.0.0.1:{stall_port}/slow.bin"}, token=token)
        check(st == 200 and ad.get("kind") == "http", "添加慢速 HTTP 任务", f"{ad}")
        tid_http = ad.get("id")
        tid_bt = None
        st, ad2 = call(port, "POST", "/api/offline/add",
                       {"dir": "下载", "url": "magnet:?xt=urn:btih:" + "1" * 40}, token=token)
        check(st == 200 and ad2.get("kind") == "bittorrent", "添加磁力任务", f"{ad2}")
        tid_bt = ad2.get("id")

        deadline = time.time() + 15
        st_http = ""
        while time.time() < deadline:
            t = task_by_id(port, token, tid_http)
            st_http = (t or {}).get("state", "")
            if st_http == "downloading":
                break
            time.sleep(0.5)
        check(st_http == "downloading", "HTTP 任务进入 downloading")

        print("\n== 强杀服务进程(模拟崩溃) ==")
        proc.kill()
        proc.wait(timeout=10)
        print("  进程已终止(SIGKILL)")

        print("\n== 重启服务 ==")
        port2 = free_port()
        proc2, ok2 = boot(BIN, data_dir, port2)
        check(ok2, "服务重启成功")
        try:
            token2 = login(port2)
            check(bool(token2), "重启后登录成功")

            t_http = task_by_id(port2, token2, tid_http)
            t_bt = task_by_id(port2, token2, tid_bt)
            err_h = (t_http or {}).get("error", "")
            check(t_http and t_http.get("state") == "failed" and "服务重启" in err_h,
                  "下载中任务重启后置为 failed", f"{t_http}")
            check(t_bt and t_bt.get("state") == "failed",
                  "解析中任务重启后置为 failed", f"{t_bt}")

            states = {(t or {}).get("state") for t in (t_http, t_bt)}
            check(not (states & {"downloading", "parsing", "awaiting_select"}),
                  "无任何任务停留在中间态", f"{states}")

            print("\n== 一键重新下载 ==")
            st, rt = call(port2, "POST", "/api/offline/retry", {"id": tid_http}, token=token2)
            check(st == 200 and rt.get("id") not in (None, tid_http), "重试创建新任务", f"{rt}")
            tid_r = rt.get("id")
            deadline = time.time() + 15
            st_r = ""
            while time.time() < deadline:
                t = task_by_id(port2, token2, tid_r)
                st_r = (t or {}).get("state", "")
                if st_r == "downloading":
                    break
                time.sleep(0.5)
            check(st_r == "downloading", "重试任务重新进入下载", f"state={st_r}")
            t_old = task_by_id(port2, token2, tid_http)
            check(t_old and t_old.get("state") == "failed", "旧失败行保留为历史", f"{t_old}")
            call(port2, "POST", "/api/offline/cancel", {"id": tid_r}, token=token2)

            st, rt2 = call(port2, "POST", "/api/offline/retry", {"id": tid_bt}, token=token2)
            check(st == 200 and rt2.get("id"), "磁力任务同样可重试", f"{rt2}")
            call(port2, "POST", "/api/offline/cancel", {"id": rt2.get("id")}, token=token2)

            st, rm = call(port2, "POST", "/api/offline/remove", {"id": tid_http}, token=token2)
            check(st == 200 and rm.get("removed") is True, "失败任务可正常删除")
        finally:
            proc2.terminate()
            try:
                proc2.wait(timeout=8)
            except Exception:
                proc2.kill()
    finally:
        try:
            proc.terminate()
            proc.wait(timeout=5)
        except Exception:
            pass
        stall.shutdown()
        shutil.rmtree(work, ignore_errors=True)

    print(f"\n==== 通过 {PASS} / 失败 {FAIL} ====")
    return 0 if FAIL == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
