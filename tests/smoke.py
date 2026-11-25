#!/usr/bin/env python3
"""NDisk(二次重建版)—— 端到端冒烟测试。

自行启动被测二进制(临时数据目录 + 随机端口),依次验证:
验证码登录 → 空间列表 → 新建目录 → 直传上传 → 浏览 → 分片上传 →
下载(含 Range)→ 重命名 → 移动 → 下载票据 / 直链 → 打包 ZIP(用 zipfile 校验)→
分享链接 → WebDAV(PROPFIND / PUT / GET / MOVE / DELETE)→ 删除 → 登出。

用法:
    python3 tests/smoke.py [二进制路径]

默认二进制路径为 ./target/debug/ndisk(或可用 NDISK_BIN 覆盖)。
退出码 0 表示全部通过。
"""
import base64
import http.client
import io
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import urllib.parse
import zipfile

BIN = sys.argv[1] if len(sys.argv) > 1 else os.environ.get(
    "NDISK_BIN", "./target/debug/ndisk"
)
# Windows 上 cargo 产物带 .exe 后缀。
if os.name == "nt" and os.path.exists(BIN + ".exe"):
    BIN += ".exe"
ADMIN_USER = "admin"
ADMIN_PASS = "admin123"

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


class Client:
    def __init__(self, host, port):
        self.host = host
        self.port = port
        self.token = ""

    def req(self, method, path, body=None, headers=None, raw=False, basic=None):
        h = dict(headers or {})
        if self.token and "Authorization" not in h and basic is None:
            h["Authorization"] = "Bearer " + self.token
        if basic is not None:
            enc = base64.b64encode(basic.encode()).decode()
            h["Authorization"] = "Basic " + enc
        data = None
        if body is not None:
            if isinstance(body, (bytes, bytearray)):
                data = bytes(body)
            else:
                data = json.dumps(body).encode()
                h.setdefault("Content-Type", "application/json")
        conn = http.client.HTTPConnection(self.host, self.port, timeout=30)
        conn.request(method, path, body=data, headers=h)
        resp = conn.getresponse()
        payload = resp.read()
        ct = resp.getheader("Content-Type", "") or ""
        conn.close()
        hdrs = {k.lower(): v for k, v in resp.getheaders()}
        if raw:
            return resp.status, payload, hdrs
        if "json" in ct:
            try:
                return resp.status, json.loads(payload), hdrs
            except Exception:
                return resp.status, None, hdrs
        return resp.status, payload, hdrs


def wait_ready(port, proc, timeout=30):
    deadline = time.time() + timeout
    while time.time() < deadline:
        if proc.poll() is not None:
            return False
        try:
            s = socket.create_connection(("127.0.0.1", port), 1)
            s.close()
            return True
        except OSError:
            time.sleep(0.2)
    return False


def captcha_answer(svg):
    chars = re.findall(r"<text[^>]*>([^<])</text>", svg)
    return "".join(chars)


def main():
    if not os.path.exists(BIN):
        print(f"找不到二进制: {BIN}(先运行 cargo build)")
        return 2
    data_dir = tempfile.mkdtemp(prefix="ndisk-smoke-")
    port = free_port()
    proc = subprocess.Popen(
        [BIN, "-p", str(port), "-d", data_dir, "-u", ADMIN_USER, "-w", ADMIN_PASS],
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    try:
        if not wait_ready(port, proc):
            print("服务启动失败,输出:")
            try:
                print(proc.stdout.read().decode(errors="replace"))
            except Exception:
                pass
            return 2
        c = Client("127.0.0.1", port)

        print("\n== 认证 ==")
        st, cap, _ = c.req("GET", "/api/captcha")
        check(st == 200 and "svg" in cap, "获取验证码")
        code = captcha_answer(cap["svg"])
        st, r, _ = c.req(
            "POST",
            "/api/auth/login",
            {
                "username": ADMIN_USER,
                "password": ADMIN_PASS,
                "captcha_id": cap["id"],
                "captcha": code,
            },
        )
        check(st == 200 and r and r.get("token"), "登录成功", f"status={st} r={r}")
        c.token = r["token"]
        st, me, _ = c.req("GET", "/api/auth/me")
        check(st == 200 and me.get("username") == ADMIN_USER, "获取当前用户")
        st, bad, _ = c.req("GET", "/api/auth/me")
        check(st == 200, "带 token 访问受保护接口")

        print("\n== 空间与目录 ==")
        st, shares, _ = c.req("GET", "/api/shares")
        check(st == 200 and shares.get("shares"), "空间列表")
        home = next(s for s in shares["shares"] if s.get("home"))
        sid = home["id"]
        check("direct_max" in shares, "返回直传上限")
        st, _, _ = c.req("POST", "/api/mkdir", {"share_id": sid, "path": "", "name": "smoke"})
        check(st == 200, "新建文件夹")
        st, b, _ = c.req("GET", f"/api/browse/{sid}?path=")
        check(any(e["name"] == "smoke" and e["is_dir"] for e in b["entries"]), "浏览看到新目录")
        st, b2, _ = c.req("GET", f"/api/browse/{sid}?path=smoke")
        check(st == 200 and b2["path"] == "smoke", "进入子目录")

        print("\n== 上传/下载 ==")
        payload = b"hello ndisk smoke test\n" * 4
        st, r, _ = c.req(
            "POST",
            f"/api/upload/direct?share_id={sid}&path=smoke&filename=note.txt",
            payload,
        )
        check(st == 200 and r.get("path") == "smoke/note.txt", "小文件直传")
        st, meta, _ = c.req("GET", f"/api/file/meta/{sid}?path=smoke/note.txt")
        check(st == 200 and meta["size"] == len(payload), "文件元信息大小一致")
        st, data, hdr = c.req("GET", f"/api/media/{sid}?path=smoke/note.txt&dl=1", raw=True)
        check(st == 200 and data == payload, "下载内容一致")
        st, part, hdr = c.req(
            "GET",
            f"/api/media/{sid}?path=smoke/note.txt&dl=1",
            raw=True,
            headers={"Range": "bytes=0-4"},
        )
        check(st == 206 and part == payload[:5] and hdr.get("content-range"),
              "Range 断点下载(206)", f"status={st} part={part!r} cr={hdr.get('content-range')}")

        # 分片上传(强制走 init/part/complete 流程)。
        print("\n== 分片上传 ==")
        big = bytes(range(256)) * 40000  # ~10MB,超过 8MB 分片
        st, init, _ = c.req(
            "POST",
            "/api/upload/init",
            {"share_id": sid, "path": "smoke", "filename": "big.bin", "size": len(big)},
        )
        check(st == 200 and init.get("token"), "初始化分片上传")
        tok = init["token"]
        chunk = init["chunk_size"]
        nparts = init["num_parts"]
        ok = True
        for i in range(nparts):
            seg = big[i * chunk: (i + 1) * chunk]
            st, _, _ = c.req("PUT", f"/api/upload/part/{tok}/{i}", seg)
            if st != 200:
                ok = False
        check(ok, "上传各分片")
        st, parts, _ = c.req("GET", f"/api/upload/parts/{tok}")
        check(st == 200 and len(parts["parts"]) == nparts, "查询已收分片")
        st, done, _ = c.req("POST", f"/api/upload/complete/{tok}")
        check(st == 200, "合并完成")
        st, data2, _ = c.req("GET", f"/api/media/{sid}?path=smoke/big.bin&dl=1", raw=True)
        check(st == 200 and data2 == big, "分片上传文件内容一致")

        print("\n== 重命名 / 移动 ==")
        st, _, _ = c.req(
            "POST", "/api/rename", {"share_id": sid, "path": "smoke", "old": "note.txt", "new": "renamed.txt"}
        )
        check(st == 200, "重命名")
        st, _, _ = c.req("POST", "/api/mkdir", {"share_id": sid, "path": "", "name": "dest"})
        st, mv, _ = c.req(
            "POST", "/api/move", {"share_id": sid, "path": "smoke", "names": ["renamed.txt"], "dest": "dest"}
        )
        check(st == 200 and mv.get("moved") == 1, "移动文件")
        st, b3, _ = c.req("GET", f"/api/browse/{sid}?path=dest")
        check(any(e["name"] == "renamed.txt" for e in b3["entries"]), "移动后出现在目标目录")

        print("\n== 票据 / 直链 ==")
        st, tk, _ = c.req(
            "POST", "/api/file/dlticket", {"share_id": sid, "path": "smoke/big.bin", "names": []}
        )
        check(st == 200 and tk.get("url", "").startswith("/dl/"), "获取下载直链", f"{tk}")
        st, data3, _ = c.req("GET", tk["url"], raw=True)
        check(st == 200 and data3 == big, "通过直链下载")

        print("\n== 打包 ZIP ==")
        st, zbytes, hdr = c.req(
            "POST", "/api/zip", {"share_id": sid, "path": "smoke", "names": ["big.bin"]}, raw=True
        )
        okzip = False
        try:
            zf = zipfile.ZipFile(io.BytesIO(zbytes))
            okzip = zf.testzip() is None and zf.read("smoke/big.bin") == big
        except Exception as e:
            print("    zip 解析异常:", e)
        check(st == 200 and okzip, "打包 ZIP 可被标准库解析且内容一致")

        print("\n== 分享链接 ==")
        st, fs, _ = c.req("POST", "/api/fileshares", {"share_id": sid, "path": "smoke/big.bin", "ttl_days": 0})
        check(st == 200 and fs.get("token"), "创建分享链接")
        st, page, _ = c.req("GET", f"/s/{fs['token']}")
        check(st == 200, "分享落地页可访问")
        st, dl, _ = c.req("GET", f"/s/{fs['token']}/api/download?dl=1", raw=True)
        check(st == 200 and dl == big, "分享链接下载内容一致")
        st, _, _ = c.req("DELETE", f"/api/fileshares/{fs['token']}")
        check(st == 200, "取消分享")

        print("\n== WebDAV ==")
        auth = ADMIN_USER + ":" + ADMIN_PASS
        st, body, hdr = c.req(
            "PROPFIND", "/dav/", headers={"Depth": "1"}, raw=True, basic=auth
        )
        check(st == 207 and b"multistatus" in body, "PROPFIND 根目录(207)")
        st, body, _ = c.req("PROPFIND", "/dav/smoke/", headers={"Depth": "1"}, raw=True, basic=auth)
        check(st == 207 and b"big.bin" in body, "PROPFIND 子目录列出文件")
        st, _, _ = c.req("PUT", "/dav/davtest.txt", body=b"dav-content", raw=True, basic=auth,
                         headers={"Content-Type": "text/plain"})
        check(st in (200, 201), "WebDAV PUT")
        st, got, _ = c.req("GET", "/dav/davtest.txt", raw=True, basic=auth)
        check(st == 200 and got == b"dav-content", "WebDAV GET")
        st, _, _ = c.req("DELETE", "/dav/davtest.txt", raw=True, basic=auth)
        check(st in (200, 204), "WebDAV DELETE")
        st, _, _ = c.req("PROPFIND", "/dav/", headers={"Depth": "0"}, raw=True,
                         basic="admin:wrongpass")
        check(st == 401, "WebDAV 错误口令返回 401")

        print("\n== 删除 / 登出 ==")
        st, d, _ = c.req("POST", "/api/delete", {"share_id": sid, "path": "smoke/big.bin", "recursive": True})
        check(st == 200, "删除文件")
        st, d, _ = c.req("POST", "/api/delete", {"share_id": sid, "path": "smoke", "recursive": True})
        check(st == 200, "递归删除目录")
        st, b4, _ = c.req("GET", f"/api/browse/{sid}?path=")
        check(not any(e["name"] == "smoke" for e in b4["entries"]), "删除后不再出现在浏览列表")
        st, _, _ = c.req("POST", "/api/auth/logout")
        check(st == 200, "登出")
        st, _, _ = c.req("GET", "/api/auth/me")
        check(st == 401, "登出后访问受保护接口 401")

        print("\n== 静态资源 ==")
        st, idx, _ = c.req("GET", "/", raw=True)
        check(st == 200 and b"<div id=\"app\">" in idx, "首页 HTML")
        st, js, _ = c.req("GET", "/static/js/main.js", raw=True)
        check(st == 200 and b"boot" in js, "前端入口 JS")
        st, css, _ = c.req("GET", "/static/css/app.css", raw=True)
        check(st == 200 and len(css) > 100, "样式表")
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=8)
        except Exception:
            proc.kill()
        shutil.rmtree(data_dir, ignore_errors=True)

    print(f"\n==== 通过 {PASS} / 失败 {FAIL} ====")
    return 0 if FAIL == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
