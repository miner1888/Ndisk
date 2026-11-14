#!/usr/bin/env bash
# 端到端验证 /api/move:登录 → 建目录/传文件 → 移动 → 冲突/自嵌套拒绝
set -euo pipefail
BASE="http://127.0.0.1:18098"
JAR=$(mktemp)
HDR="Authorization: Bearer"

req() { # method path [json-body]
  local m=$1 p=$2 b=${3:-}
  if [ -n "$b" ]; then
    curl -sS -X "$m" "$BASE$p" -b "$JAR" -c "$JAR" -H 'Content-Type: application/json' -d "$b"
  else
    curl -sS -X "$m" "$BASE$p" -b "$JAR" -c "$JAR"
  fi
}

# 1. 验证码(4 个 <text> 字符按序渲染在 SVG 里)
CAP=$(req GET /api/captcha)
CID=$(jq -r .id <<<"$CAP")
CODE=$(grep -o '<text[^>]*>[^<]*</text>' <<<"$CAP" | sed 's/.*>\([^<]*\)<.*/\1/' | tr -d '\n')
echo "captcha: id=$CID code=$CODE"

# 2. 登录
LOGIN=$(req POST /api/auth/login "{\"username\":\"admin\",\"password\":\"admin123\",\"captcha_id\":\"$CID\",\"captcha\":\"$CODE\"}")
TOKEN=$(jq -r .token <<<"$LOGIN")
req() { # 重新定义:附带 Bearer
  local m=$1 p=$2 b=${3:-}
  if [ -n "$b" ]; then
    curl -sS -X "$m" "$BASE$p" -b "$JAR" -c "$JAR" -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' -d "$b"
  else
    curl -sS -X "$m" "$BASE$p" -b "$JAR" -c "$JAR" -H "Authorization: Bearer $TOKEN"
  fi
}
echo "login: user=$(jq -r '.user.username' <<<"$LOGIN")"

# 3. 找到管理员个人空间,建 src/dst
SHARES=$(req GET /api/shares)
SHARE=$(jq '[.shares[] | select(.kind=="home" and .mine)][0].id' <<<"$SHARES")
echo "home share id=$SHARE"
req POST /api/mkdir "{\"share_id\":$SHARE,\"path\":\"\",\"name\":\"dst\"}" >/dev/null
req POST /api/mkdir "{\"share_id\":$SHARE,\"path\":\"\",\"name\":\"mvdir\"}" >/dev/null

# 4. 分片上传一个小文件到根目录
INIT=$(req POST /api/upload/init "{\"share_id\":$SHARE,\"path\":\"\",\"filename\":\"hello.txt\",\"size\":5}")
T=$(jq -r .token <<<"$INIT"); CS=$(jq -r .chunk_size <<<"$INIT"); NP=$(jq -r .num_parts <<<"$INIT")
printf 'hello' > /tmp/ndisk-part.bin
curl -sS -X PUT "$BASE/api/upload/part/$T/0" -b "$JAR" -H "Authorization: Bearer $TOKEN" --data-binary @/tmp/ndisk-part.bin >/dev/null
req POST "/api/upload/complete/$T" >/dev/null
echo "uploaded hello.txt (chunk=$CS parts=$NP)"

# 5. 移动文件 + 文件夹到 dst/
echo "-- move [hello.txt, mvdir] -> dst"
req POST /api/move "{\"share_id\":$SHARE,\"path\":\"\",\"names\":[\"hello.txt\",\"mvdir\"],\"dest\":\"dst\"}"
echo

# 6. dst 里应有两个条目
B=$(req GET "/api/browse/$SHARE?path=dst")
echo "dst entries: $(jq -c '[.entries[].name]' <<<"$B")"
[ "$(jq '.entries | length' <<<"$B")" = "2" ] || { echo "FAIL: dst should have 2 entries"; exit 1; }

# 7. 目标同名冲突
req POST /api/mkdir "{\"share_id\":$SHARE,\"path\":\"\",\"name\":\"hello.txt\"}" >/dev/null 2>&1 || true
echo "hello text" > /tmp/x; INIT=$(req POST /api/upload/init "{\"share_id\":$SHARE,\"path\":\"\",\"filename\":\"hello.txt\",\"size\":11}")
T=$(jq -r .token <<<"$INIT")
curl -sS -X PUT "$BASE/api/upload/part/$T/0" -b "$JAR" -H "Authorization: Bearer $TOKEN" --data-binary @/tmp/x >/dev/null
req POST "/api/upload/complete/$T" >/dev/null
echo "-- conflict: hello.txt -> dst (expect error)"
req POST /api/move "{\"share_id\":$SHARE,\"path\":\"\",\"names\":[\"hello.txt\"],\"dest\":\"dst\"}"
echo

# 8. 文件夹移动到自己内部( expect error)
echo "-- self-nest: dst/mvdir -> dst/mvdir/sub (expect error)"
req POST /api/move "{\"share_id\":$SHARE,\"path\":\"dst/mvdir\",\"names\":[\"mvdir\"],\"dest\":\"dst/mvdir/sub\"}"
echo

echo "SMOKE OK"
