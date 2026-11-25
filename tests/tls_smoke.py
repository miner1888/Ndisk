#!/usr/bin/env python3
"""NDisk(二次重建版)—— HTTPS(证书热启用)端到端验证。

用 openssl 生成自签名证书,经 /api/tls 上传后,校验:
  - 服务在配置证书后热启动 HTTPS 监听(无需重启)
  - 通过 TLS 访问 API(/api/config)与鉴权接口(/api/auth/me)成功
  - 原有 HTTP 访问不受影响
  - 删除证书后 HTTPS 监听被关闭
用法:python3 tests/tls_smoke.py [二进制路径]
"""
import http.client
import json
import os
import re
import shutil
import socket
import ssl
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


def http_call(port, method, path, body=None, token="", tls=False, timeout=20):
    hdr = {}
    if token:
        hdr["Authorization"] = "Bearer " + token
    data = None
    if body is not None:
        data = json.dumps(body).encode()
        hdr["Content-Type"] = "application/json"
    if tls:
        ctx = ssl.create_default_context()
        ctx.check_hostname = False
        ctx.verify_mode = ssl.CERT_NONE
        conn = http.client.HTTPSConnection("127.0.0.1", port, timeout=timeout, context=ctx)
    else:
        conn = http.client.HTTPConnection("127.0.0.1", port, timeout=timeout)
    conn.request(method, path, body=data, headers=hdr)
    r = conn.getresponse()
    payload = r.read()
    ct = r.getheader("Content-Type", "") or ""
    conn.close()
    if "json" in ct:
        try:
            return r.status, json.loads(payload)
        except Exception:
            return r.status, None
    return r.status, payload


def wait_port(port, proc, timeout=20, tls=False):
    deadline = time.time() + timeout
    while time.time() < deadline:
        if proc.poll() is not None:
            return False
        try:
            s = socket.create_connection(("127.0.0.1", port), 1)
            s.close()
            if tls:
                ctx = ssl.create_default_context()
                ctx.check_hostname = False
                ctx.verify_mode = ssl.CERT_NONE
                raw = socket.create_connection(("127.0.0.1", port), 2)
                try:
                    ctx.wrap_socket(raw, server_hostname="localhost").close()
                    return True
                except ssl.SSLError:
                    time.sleep(0.3)
                    continue
            else:
                return True
        except OSError:
            time.sleep(0.2)
    return False


def captcha_answer(svg):
    return "".join(re.findall(r"<text[^>]*>([^<])</text>", svg))


def gen_self_signed(cert_path, key_path):
    """无 openssl 时用 Python cryptography 生成自签名证书(CN=localhost)。"""
    from cryptography import x509
    from cryptography.hazmat.primitives import hashes, serialization
    from cryptography.hazmat.primitives.asymmetric import rsa
    from cryptography.x509.oid import NameOID
    import datetime

    key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    subject = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "localhost")])
    now = datetime.datetime.now(datetime.timezone.utc)
    cert = (
        x509.CertificateBuilder()
        .subject_name(subject)
        .issuer_name(subject)
        .public_key(key.public_key())
        .serial_number(x509.random_serial_number())
        .not_valid_before(now - datetime.timedelta(days=1))
        .not_valid_after(now + datetime.timedelta(days=1))
        .add_extension(x509.SubjectAlternativeName([x509.DNSName("localhost")]), critical=False)
        .sign(key, hashes.SHA256())
    )
    with open(cert_path, "wb") as f:
        f.write(cert.public_bytes(serialization.Encoding.PEM))
    with open(key_path, "wb") as f:
        f.write(key.private_bytes(
            serialization.Encoding.PEM,
            serialization.PrivateFormat.PKCS8,
            serialization.NoEncryption(),
        ))


def main():
    if not os.path.exists(BIN):
        print(f"找不到二进制: {BIN}")
        return 2
    has_openssl = bool(shutil.which("openssl"))
    has_crypto = False
    if not has_openssl:
        try:
            import cryptography  # noqa: F401
            has_crypto = True
        except ImportError:
            pass
    if not has_openssl and not has_crypto:
        print("本机无 openssl 且无 python cryptography,跳过 TLS 测试")
        return 0
    work = tempfile.mkdtemp(prefix="ndisk-tls-")
    data_dir = os.path.join(work, "data")
    cert_pem = os.path.join(work, "cert.pem")
    key_pem = os.path.join(work, "key.pem")
    # 自签名证书(CN=localhost):优先 openssl,缺失时用 python cryptography。
    if has_openssl:
        subprocess.run(
            ["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes",
             "-keyout", key_pem, "-out", cert_pem, "-days", "1",
             "-subj", "/CN=localhost"],
            check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
    else:
        gen_self_signed(cert_pem, key_pem)
    http_port = free_port()
    tls_port = free_port()
    proc = subprocess.Popen(
        [BIN, "-p", str(http_port), "--tls-listen", f"127.0.0.1:{tls_port}", "-d", data_dir],
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
    )
    try:
        if not wait_port(http_port, proc):
            print("HTTP 启动失败")
            print(proc.stdout.read().decode(errors="replace"))
            return 2

        print("\n== 登录并上传证书 ==")
        st, cap = http_call(http_port, "GET", "/api/captcha")
        code = captcha_answer(cap["svg"])
        st, r = http_call(http_port, "POST", "/api/auth/login",
                          {"username": "admin", "password": "admin123",
                           "captcha_id": cap["id"], "captcha": code})
        check(st == 200 and r.get("token"), "登录成功")
        token = r["token"]

        st, before = http_call(http_port, "GET", "/api/tls", token=token)
        check(st == 200 and before.get("enabled") is False, "初始未启用 HTTPS")

        st, put = http_call(http_port, "PUT", "/api/tls",
                            {"domain": "localhost", "cert_path": cert_pem, "key_path": key_pem},
                            token=token)
        check(st == 200, "上传证书(文件路径方式)", f"status={st} {put}")

        check(wait_port(tls_port, proc, timeout=15, tls=True), "HTTPS 热启动并完成 TLS 握手(无需重启)")

        print("\n== TLS 访问 ==")
        st, cfg = http_call(tls_port, "GET", "/api/config", tls=True)
        check(st == 200 and "chunk_size" in cfg, "TLS 访问 /api/config")
        st, me = http_call(tls_port, "GET", "/api/auth/me", token=token, tls=True)
        check(st == 200 and me.get("username") == "admin", "TLS 访问鉴权接口 /api/auth/me")
        st, after = http_call(http_port, "GET", "/api/tls", token=token)
        check(st == 200 and after.get("enabled") is True and after.get("port") == tls_port,
              "状态显示 HTTPS 已启用")

        print("\n== 原有 HTTP 不受影响 ==")
        st, cfg2 = http_call(http_port, "GET", "/api/config")
        check(st == 200 and "chunk_size" in cfg2, "HTTP 仍正常")

        print("\n== 删除证书后停用 ==")
        st, _ = http_call(http_port, "DELETE", "/api/tls", token=token)
        check(st == 200, "删除证书")
        time.sleep(1.5)
        st, after2 = http_call(http_port, "GET", "/api/tls", token=token)
        check(st == 200 and after2.get("enabled") is False, "状态显示 HTTPS 已停用")
        https_dead = False
        try:
            http_call(tls_port, "GET", "/api/config", tls=True, timeout=3)
        except Exception:
            https_dead = True
        check(https_dead, "HTTPS 端口不再接受连接")
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
