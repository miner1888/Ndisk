#!/usr/bin/env bash
# 实测:转码替换开关 / 上传 .avi 自动转 MP4 替换 / 相册包含视频
set -euo pipefail
B=http://127.0.0.1:18098
JAR=$(mktemp)
CAP=$(curl -sS $B/api/captcha); CID=$(jq -r .id <<<"$CAP"); CODE=$(grep -o '<text[^>]*>[^<]*</text>' <<<"$CAP" | sed 's/.*>\([^<]*\)<.*/\1/' | tr -d '\n')
LOGIN=$(curl -sS -X POST $B/api/auth/login -b $JAR -c $JAR -H 'Content-Type: application/json' -d "{\"username\":\"admin\",\"password\":\"admin123\",\"captcha_id\":\"$CID\",\"captcha\":\"$CODE\"}")
T=$(jq -r .token <<<"$LOGIN"); A="Authorization: Bearer $T"

echo "-- 默认开关状态"
curl -sS $B/api/admin/status -b $JAR -H "$A" | jq '.transcode_replace'

echo "-- 开启"
curl -sS -X POST $B/api/admin/transcode-replace -b $JAR -H "$A" -H 'Content-Type: application/json' -d '{"enabled":true}'
echo
curl -sS $B/api/admin/status -b $JAR -H "$A" | jq '.transcode_replace'

echo "-- 生成 2 秒测试视频 clip.avi 并上传"
rm -f /tmp/clip.avi
ffmpeg -hide_banner -loglevel error -y -f lavfi -i testsrc=duration=2:size=320x240:rate=15 -pix_fmt yuv420p /tmp/clip.avi
ls -la /tmp/clip.avi
head -c 12 /tmp/clip.avi | od -c | head -1
INIT=$(curl -sS -X POST $B/api/upload/init -b $JAR -H "$A" -H 'Content-Type: application/json' -d '{"share_id":1,"path":"","filename":"clip.avi","size":'"$(stat -c%s /tmp/clip.avi)"'}')
TOK=$(jq -r .token <<<"$INIT")
curl -sS -X PUT "$B/api/upload/part/$TOK/0" -b $JAR -H "$A" --data-binary @/tmp/clip.avi > /dev/null
curl -sS -X POST "$B/api/upload/complete/$TOK" -b $JAR -H "$A"
echo

echo "-- 轮询等待后台转码替换(最多 60s)"
for i in $(seq 1 30); do
  sleep 2
  MAGIC=$(head -c 12 /tmp/ndisk-ui-test/users/admin/clip.avi 2>/dev/null | od -An -c | tr -d ' \n' || true)
  echo "[$i] 头部: $MAGIC"
  if [[ "$MAGIC" == *f*t*y*p* || "$MAGIC" == *1A45DFA3* ]]; then
    echo "已替换为 MP4 容器!"
    break
  fi
done
ls -la /tmp/ndisk-ui-test/users/admin/clip.avi

echo "-- 相册应包含视频条目"
curl -sS "$B/api/album/1" -b $JAR -H "$A" | jq -c '.images[] | {name, kind}'

echo "-- 关闭开关"
curl -sS -X POST $B/api/admin/transcode-replace -b $JAR -H "$A" -H 'Content-Type: application/json' -d '{"enabled":false}'
echo
curl -sS $B/api/admin/status -b $JAR -H "$A" | jq '.transcode_replace'
