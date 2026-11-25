#!/usr/bin/env python3
"""NDisk(二次重建版)—— 权限模型端到端验证。

验证:
  - 个人空间(私有)对游客不可见、直连共享 ID 也拿不到
  - 共享目录按根规则 guest/login/admin 生效(游客只读)
  - 深路径规则覆盖上层规则
  - 账号能力开关(禁下载后下载被拒)
用法:python3 tests/permissions_smoke.py [二进制路径]
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
    c = http.client.HTTPConnection("127.0.0.1", port, timeout=20)
    c.request(method, path, body=data, headers=h)
    r = c.getresponse()
    pl = r.read()
    ct = r.getheader("Content-Type", "") or ""
    c.close()
    if raw:
        return r.status, pl
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


def login(port, user, pw):
    st, cap = call(port, "GET", "/api/captcha")
    code = captcha_answer(cap["svg"])
    st, r = call(port, "POST", "/api/auth/login",
                 {"username": user, "password": pw,
                  "captcha_id": cap["id"], "captcha": code})
    return r.get("token") if st == 200 and r else None


def main():
    if not os.path.exists(BIN):
        print(f"找不到二进制: {BIN}")
        return 2
    work = tempfile.mkdtemp(prefix="ndisk-perm-")
    data_dir = os.path.join(work, "data")
    shared_dir = os.path.join(work, "shared")
    os.makedirs(os.path.join(shared_dir, "pub"), exist_ok=True)
    with open(os.path.join(shared_dir, "pub", "hello.txt"), "wb") as f:
        f.write(b"shared hello")
    port = free_port()
    proc = subprocess.Popen([BIN, "-p", str(port), "-d", data_dir],
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    try:
        if not wait_ready(port, proc):
            print("启动失败")
            print(proc.stdout.read().decode(errors="replace"))
            return 2
        admin = login(port, "admin", "admin123")
        check(admin is not None, "管理员登录")
        st, shares = call(port, "GET", "/api/shares", token=admin)
        home = next(s for s in shares["shares"] if s.get("home"))
        home_id = home["id"]

        print("\n== 个人空间私有 ==")
        st, anon = call(port, "GET", "/api/shares")
        check(st == 200 and all(not s.get("home") for s in anon["shares"]),
              "游客看不到任何个人空间")
        st, _ = call(port, "GET", f"/api/browse/{home_id}?path=")
        check(st in (401, 403), "游客直连个人空间 ID 被拒", f"status={st}")
        st, _ = call(port, "GET", f"/api/media/{home_id}?path=x&dl=1")
        check(st in (401, 403), "游客直连个人空间下载被拒", f"status={st}")

        print("\n== 共享目录:根规则 guest ==")
        st, cr = call(port, "POST", "/api/shares",
                      {"name": "公共区", "host_path": shared_dir}, token=admin)
        check(st == 200 and cr.get("id"), "管理员创建共享目录")
        sid = cr["id"]
        st, _ = call(port, "POST", f"/api/shares/{sid}/rules",
                     {"rel_path": "", "access": "guest"}, token=admin)
        check(st == 200, "根目录设为 guest")
        st, b = call(port, "GET", f"/api/browse/{sid}?path=")
        check(st == 200, "游客可浏览 guest 目录", f"status={st}")
        st, b2 = call(port, "GET", f"/api/browse/{sid}?path=pub")
        check(st == 200 and any(e["name"] == "hello.txt" for e in (b2 or {}).get("entries", [])),
              "游客看到子目录内容")
        # 游客只读
        st, _ = call(port, "POST", "/api/mkdir", {"share_id": sid, "path": "", "name": "x"})
        check(st in (401, 403), "游客不能新建目录", f"status={st}")

        print("\n== 深路径规则覆盖 ==")
        st, _ = call(port, "POST", f"/api/shares/{sid}/rules",
                     {"rel_path": "pub", "access": "admin"}, token=admin)
        check(st == 200, "为 pub 设置 admin 规则")
        st, _ = call(port, "GET", f"/api/browse/{sid}?path=pub")
        check(st in (401, 403), "游客访问 admin 子目录被拒", f"status={st}")
        st, b = call(port, "GET", f"/api/browse/{sid}?path=")
        check(st == 200, "根目录仍为 guest 可访问")

        print("\n== 根设为 admin ==")
        st, _ = call(port, "POST", f"/api/shares/{sid}/rules",
                     {"rel_path": "", "access": "admin"}, token=admin)
        st, _ = call(port, "GET", f"/api/browse/{sid}?path=")
        check(st in (401, 403), "根设为 admin 后游客被拒", f"status={st}")

        print("\n== 账号能力开关 ==")
        st, u = call(port, "POST", "/api/users",
                     {"username": "bob", "password": "bob12345",
                      "permit": {"can_upload": True, "can_download": False,
                                 "can_mkdir": True, "can_delete": False}},
                     token=admin)
        check(st == 200, "创建用户 bob(禁下载)")
        bob = login(port, "bob", "bob12345")
        check(bob is not None, "bob 登录")
        st, _ = call(port, "POST", f"/api/shares/{sid}/rules",
                     {"rel_path": "", "access": "login"}, token=admin)
        # bob 上传到共享目录(有 can_upload)
        st, up = call(port, "POST",
                      f"/api/upload/direct?share_id={sid}&path=&filename=bob.txt",
                      b"from bob", token=bob)
        check(st == 200, "bob 可上传(有上传权限)")
        st, _ = call(port, "GET", f"/api/media/{sid}?path=bob.txt&dl=1", token=bob)
        check(st == 403, "bob 下载被拒(禁下载)", f"status={st}")
        st, _ = call(port, "GET", f"/api/media/{sid}?path=bob.txt&dl=1")
        check(st in (401, 403), "游客在 login 目录被拒", f"status={st}")
        st, d = call(port, "GET", f"/api/media/{sid}?path=bob.txt&dl=1", token=admin)
        check(st == 200 and d == b"from bob", "管理员可下载")
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
