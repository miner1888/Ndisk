#!/usr/bin/env python3
"""NDisk(二次重建版)—— eD2K(电驴)下载端到端验证(回环、确定性)。

公网 eD2K 服务器在沙箱内不可达,故用两个本地 mock 复现真实协议:
  - mock eD2K 服务器:登录应答 + GetSources 返回本地 mock 对端地址
  - mock eD2K 对端:HELLO/文件协商/上传槽/hashset/分片请求 → 回送数据
以文件大小 2*PART_SIZE+5000 强制走 hashset + 多块并行 + 逐块与根 MD4 校验。

用法:python3 tests/ed2k_local_smoke.py [二进制路径]
"""
import http.client
import json
import os
import re
import shutil
import socket
import socketserver
import struct
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
PART_SIZE = 9_728_000
PROTO_EDONKEY = 0xE3

PAYLOAD = os.urandom(2 * PART_SIZE + 5000)
PARTS = [PAYLOAD[0:PART_SIZE], PAYLOAD[PART_SIZE:2 * PART_SIZE], PAYLOAD[2 * PART_SIZE:]]
ROOT_HASH = b""  # 由 main 计算


def check(cond, label, extra=""):
    global PASS, FAIL
    if cond:
        PASS += 1
        print(f"  \033[32mPASS\033[0m {label}")
    else:
        FAIL += 1
        print(f"  \033[31mFAIL\033[0m {label} {extra}")


def md4(msg):
    def lrot(x, n):
        return ((x << n) | (x >> (32 - n))) & 0xFFFFFFFF

    ml = len(msg) * 8
    buf = bytearray(msg)
    buf.append(0x80)
    while len(buf) % 64 != 56:
        buf.append(0)
    buf += struct.pack("<Q", ml)
    a, b, c, d = 0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476
    for off in range(0, len(buf), 64):
        X = list(struct.unpack("<16I", buf[off:off + 64]))
        A, B, C, D = a, b, c, d
        for i in range(16):
            F = (B & C) | (~B & D)
            A = lrot((A + F + X[i]) & 0xFFFFFFFF, [3, 7, 11, 19][i % 4])
            A, B, C, D = D, A, B, C
        X2 = [0, 4, 8, 12, 1, 5, 9, 13, 2, 6, 10, 14, 3, 7, 11, 15]
        for i in range(16):
            G = (B & C) | (B & D) | (C & D)
            A = lrot((A + G + X[X2[i]] + 0x5A827999) & 0xFFFFFFFF, [3, 5, 9, 13][i % 4])
            A, B, C, D = D, A, B, C
        X3 = [0, 8, 4, 12, 2, 10, 6, 14, 1, 9, 5, 13, 3, 11, 7, 15]
        for i in range(16):
            H = B ^ C ^ D
            A = lrot((A + H + X[X3[i]] + 0x6ED9EBA1) & 0xFFFFFFFF, [3, 9, 11, 15][i % 4])
            A, B, C, D = D, A, B, C
        a = (a + A) & 0xFFFFFFFF
        b = (b + B) & 0xFFFFFFFF
        c = (c + C) & 0xFFFFFFFF
        d = (d + D) & 0xFFFFFFFF
    return struct.pack("<4I", a, b, c, d)


PART_HASHES = [md4(p) for p in PARTS]
FILE_HASH = md4(b"".join(PART_HASHES))
PC = len(PARTS)
# 记录 mock 对端从客户端收到的操作码,用于断言"客户端只请求、不上传/共享"。
PEER_OPS = set()


def frame(proto, op, payload=b""):
    return bytes([proto]) + struct.pack("<I", len(payload) + 1) + bytes([op]) + payload


def recvn(conn, n):
    buf = b""
    while len(buf) < n:
        chunk = conn.recv(n - len(buf))
        if not chunk:
            raise EOFError
        buf += chunk
    return buf


def read_frame(conn):
    head = recvn(conn, 5)
    proto = head[0]
    ln = struct.unpack("<I", head[1:5])[0]
    body = recvn(conn, ln)
    return proto, body[0], body[1:]


class Ed2kServer(socketserver.BaseRequestHandler):
    """极简 eD2K 服务器:登录 + 查源。"""

    def handle(self):
        c = self.request
        try:
            read_frame(c)  # LOGIN_REQ
            c.sendall(frame(PROTO_EDONKEY, 0x40, struct.pack("<I", 1)))  # ID_CHANGE
            proto, op, payload = read_frame(c)  # GET_SOURCES
            if op == 0x19 and len(payload) >= 16:
                h = payload[:16]
                ip = bytes([127, 0, 0, 1])
                entry = ip + struct.pack("<H", self.server.peer_port)
                outp = h + struct.pack("<I", 1) + entry
                c.sendall(frame(PROTO_EDONKEY, 0x42, outp))  # FOUND_SOURCES
            time.sleep(0.2)
        except (EOFError, OSError):
            pass


class Ed2kPeer(socketserver.BaseRequestHandler):
    """极简 eD2K 对端:握手 → 文件协商 → 上传槽 → hashset → 分片。"""

    def handle(self):
        c = self.request
        try:
            _, op, _ = read_frame(c)  # HELLO
            PEER_OPS.add(op)
            c.sendall(frame(PROTO_EDONKEY, 0x4C, b""))  # HELLOANSWER
            # 等待 FileRequest(0x58);期间可能收到其它帧
            while True:
                _, op, payload = read_frame(c)
                PEER_OPS.add(op)
                if op == 0x58:  # FileRequest
                    bitfield = bytes([0xFF] * ((PC + 7) // 8))
                    c.sendall(frame(PROTO_EDONKEY, 0x50,
                                    FILE_HASH + struct.pack("<H", PC) + bitfield))  # FILESTATUS
                    break
            # 等待 STARTUPLOADREQ
            while True:
                _, op, payload = read_frame(c)
                PEER_OPS.add(op)
                if op == 0x54:  # STARTUPLOADREQ
                    c.sendall(frame(PROTO_EDONKEY, 0x55, b""))  # ACCEPTUPLOADREQ
                    break
            # 处理 hashset 与分片请求
            while True:
                _, op, payload = read_frame(c)
                PEER_OPS.add(op)
                if op == 0x51:  # HASHSETREQUEST
                    body = FILE_HASH + struct.pack("<H", PC) + b"".join(PART_HASHES)
                    c.sendall(frame(PROTO_EDONKEY, 0x52, body))  # HASHSETANSWER
                elif op == 0x47:  # REQUESTPARTS
                    starts = struct.unpack("<3I", payload[16:28])
                    ends = struct.unpack("<3I", payload[28:40])
                    for s, e in zip(starts, ends):
                        if s < e:
                            data = PAYLOAD[s:e]
                            outp = FILE_HASH + struct.pack("<I", s) + struct.pack("<I", e) + data
                            c.sendall(frame(PROTO_EDONKEY, 0x46, outp))  # SENDINGPART
        except (EOFError, OSError):
            pass


class ThreadedTCP(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
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
    if not os.path.exists(BIN):
        print(f"找不到二进制: {BIN}")
        return 2
    # 自检 MD4(已知向量)
    assert md4(b"abc").hex() == "a448017aaf21d8525fc10ae87aa6729d", "MD4 自检失败"

    peer_port = free_port()
    srv = ThreadedTCP(("127.0.0.1", 0), Ed2kServer)
    srv.peer_port = peer_port
    srv_port = srv.server_address[1]
    peer = ThreadedTCP(("127.0.0.1", peer_port), Ed2kPeer)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    threading.Thread(target=peer.serve_forever, daemon=True).start()

    work = tempfile.mkdtemp(prefix="ndisk-ed2k-")
    data_dir = os.path.join(work, "data")
    os.makedirs(data_dir, exist_ok=True)
    # 预置服务器缓存,避免引擎去公网拉 server.met(沙箱不可达会拖慢)
    with open(os.path.join(data_dir, "ed2k-servers.txt"), "w") as f:
        f.write(f"127.0.0.1:{srv_port}\n")

    api_port = free_port()
    proc = subprocess.Popen([BIN, "-p", str(api_port), "-d", data_dir],
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    try:
        if not wait_ready(api_port, proc):
            print("启动失败")
            print(proc.stdout.read().decode(errors="replace"))
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

        # 关掉 KAD,指向 mock 服务器
        st, _ = call(api_port, "POST", "/api/admin/offline-settings",
                     {"ed2k_servers": f"127.0.0.1:{srv_port}", "kad_enabled": False},
                     token=token)
        check(st == 200, "配置 eD2K 服务器并关闭 KAD")

        link = ("ed2k://|file|payload.bin|%d|%s|/"
                % (len(PAYLOAD), FILE_HASH.hex().upper()))
        print(f"\n== eD2K 下载(size={len(PAYLOAD)}, 块数={PC}) ==")
        st, ad = call(api_port, "POST", "/api/offline/add",
                      {"dir": "下载", "url": link}, token=token)
        check(st == 200 and ad.get("kind") == "ed2k", "添加 eD2K 任务", f"{ad}")
        tid = ad.get("id")

        states = []
        done = False
        deadline = time.time() + 180
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
        check(done, "eD2K 下载 completed", f"{last}")

        print("\n== 落盘与内容校验(逐块 + 根 MD4)== ")
        st, b = call(api_port, "GET", f"/api/browse/{sid}?path=%E4%B8%8B%E8%BD%BD", token=token)
        names = [e["name"] for e in b.get("entries", [])]
        check("payload.bin" in names, "文件出现在 下载 目录", f"{names}")
        st, body = call(api_port, "GET",
                        f"/api/media/{sid}?path=%E4%B8%8B%E8%BD%BD/payload.bin&dl=1", token=token)
        check(st == 200 and body == PAYLOAD, "下载内容与源逐字节一致(含根哈希校验通过)")
        dl_parts = [body[0:PART_SIZE], body[PART_SIZE:2 * PART_SIZE], body[2 * PART_SIZE:]]
        check(md4(b"".join(md4(p) for p in dl_parts)) == FILE_HASH,
              "按 eD2K 规则重算根哈希(块哈希拼接的 MD4)与链接一致")

        print("\n== eD2K 不上传 / 不共享 ==")
        allowed = {0x01, 0x58, 0x4F, 0x54, 0x51, 0x47}
        server_ops = {0x46, 0x52, 0x55, 0x4C, 0x50, 0x48}
        check(bool(PEER_OPS) and PEER_OPS <= allowed, "客户端只发送请求类操作码", f"{sorted(PEER_OPS)}")
        check(not (PEER_OPS & server_ops), "客户端从未发送数据/上传类帧(不向他人共享文件)",
              f"{sorted(PEER_OPS)}")
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=8)
        except Exception:
            proc.kill()
        srv.shutdown()
        peer.shutdown()
        shutil.rmtree(work, ignore_errors=True)

    print(f"\n==== 通过 {PASS} / 失败 {FAIL} ====")
    return 0 if FAIL == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
