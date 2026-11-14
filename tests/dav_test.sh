#!/usr/bin/env bash
# WebDAV 端到端:PROPFIND / PUT / GET / Range / MKCOL / COPY / MOVE / DELETE / 权限
set -euo pipefail
B=http://127.0.0.1:18098
DAV="http://127.0.0.1:18098/dav"
JAR=$(mktemp)

CAP=$(curl -sS $B/api/captcha); CID=$(jq -r .id <<<"$CAP"); CODE=$(grep -o '<text[^>]*>[^<]*</text>' <<<"$CAP" | sed 's/.*>\([^<]*\)<.*/\1/' | tr -d '\n')
LOGIN=$(curl -sS -X POST $B/api/auth/login -c $JAR -b $JAR -H 'Content-Type: application/json' -d "{\"username\":\"admin\",\"password\":\"admin123\",\"captcha_id\":\"$CID\",\"captcha\":\"$CODE\"}")
T=$(jq -r .token <<<"$LOGIN"); BEARER="Authorization: Bearer $T"

echo "== 1. OPTIONS(DAV 头) =="
curl -sS -X OPTIONS $DAV/ -b $JAR -H "$BEARER" -D- -o /dev/null | grep -iE "^dav|^allow"

echo "== 2. 无凭证 PROPFIND → 401 =="
curl -sS -X PROPFIND $DAV/ -o /dev/null -w "%{http_code}\n"

echo "== 3. 错误密码 → 401 =="
curl -sS -X PROPFIND $DAV/ -u admin:wrongpass -o /dev/null -w "%{http_code}\n"

echo "== 4. PROPFIND 根 → 207 空间列表 =="
curl -sS -X PROPFIND $DAV/ -b $JAR -H "$BEARER" | grep -o "<D:displayname>[^<]*</D:displayname>" | head -3

echo "== 5. MKCOL 建目录 =="
curl -sS -X MKCOL $DAV/公共区/webdav -b $JAR -H "$BEARER" -o /dev/null -w "%{http_code}\n"

echo "== 6. PUT 文件(深路径自动建目录) =="
echo "webdav hello 内容A" > /tmp/dav-a.txt
curl -sS -T /tmp/dav-a.txt "$DAV/公共区/webdav/深/目录/a.txt" -b $JAR -H "$BEARER" -o /dev/null -w "%{http_code}\n"

echo "== 7. GET 回读 + Range(断点续传)=="
curl -sS "$DAV/公共区/webdav/深/目录/a.txt" -b $JAR -H "$BEARER" -H "Range: bytes=0-10" -o /tmp/dav-part -w "Range: %{http_code} %{size_download}B / "
head -c 11 /tmp/dav-a.txt; echo

echo "== 8. COPY(Destination 已按 URL 编码,符合真实客户端) =="
ENC1=$(python3 -c "import urllib.parse;print(urllib.parse.quote('公共区/webdav/a-copy.txt'))")
curl -sS -X COPY $DAV/公共区/webdav/深/目录/a.txt -b $JAR -H "$BEARER" -H "Destination: $DAV/$ENC1" -o /dev/null -w "%{http_code}\n"

echo "== 9. MOVE(重命名) =="
ENC2=$(python3 -c "import urllib.parse;print(urllib.parse.quote('公共区/webdav/a-renamed.txt'))")
curl -sS -X MOVE $DAV/公共区/webdav/a-copy.txt -b $JAR -H "$BEARER" -H "Destination: $DAV/$ENC2" -o /dev/null -w "%{http_code}\n"

echo "== 10. DELETE =="
curl -sS -X DELETE $DAV/公共区/webdav/a-renamed.txt -b $JAR -H "$BEARER" -o /dev/null -w "%{http_code}\n"

echo "== 11. LOCK =="
curl -sS -X LOCK $DAV/公共区/webdav/深/目录/a.txt -b $JAR -H "$BEARER" | grep -o "<D:locktoken><D:href>[^<]*" | head -1

echo "== 12. 网页端可见性一致 =="
curl -sS "$B/api/browse/2?path=webdav" -b $JAR -H "$BEARER" | jq -c '[.entries[].name]'
echo "DAV TEST DONE"
