#!/usr/bin/env bash
# 并发内存压测:多会话并发上传 / 浏览 / 大目录打包下载,采样 RSS 是否无界增长
set -euo pipefail
B=http://127.0.0.1:18098
DATA=/tmp/ndisk-memtest-r1
mkdir -p $DATA/users/admin/big

echo "-- 准备测试数据(30MB 大文件 + 100 个小文件)"
if [ ! -f $DATA/users/admin/big/big.bin ] || [ $(stat -c%s $DATA/users/admin/big/big.bin) != 31457280 ]; then
  head -c 31457280 /dev/urandom > $DATA/users/admin/big/big.bin
  for i in $(seq 1 100); do head -c 4096 /dev/urandom > $DATA/users/admin/small_$i.bin; done
fi

login() {
  local CAP=$(curl -sS $B/api/captcha)
  local CID=$(jq -r .id <<<"$CAP"); local CODE=$(grep -o '<text[^>]*>[^<]*</text>' <<<"$CAP" | sed 's/.*>\([^<]*\)<.*/\1/' | tr -d '\n')
  curl -sS -X POST $B/api/auth/login -c "$1" -b "$1" -H 'Content-Type: application/json' \
    -d "{\"username\":\"admin\",\"password\":\"admin123\",\"captcha_id\":\"$CID\",\"captcha\":\"$CODE\"}" | jq -r .token
}
T1=$(login /tmp/m1.jar); T2=$(login /tmp/m2.jar)
H1="Authorization: Bearer $T1"; H2="Authorization: Bearer $T2"
echo "-- 两个会话登录完成"

PID=${PID:?需要通过环境变量传入服务进程 PID}
rss() { awk '/VmRSS/{print $2}' /proc/$PID/status; }
echo "起始 RSS: $(rss) KB"

upload_one() { # jar auth file name
  local SIZE=$(stat -c%s "$3")
  local INIT=$(curl -sS -X POST $B/api/upload/init -b "$1" -H "$2" -H 'Content-Type: application/json' -d "{\"share_id\":1,\"path\":\"\",\"filename\":\"$4\",\"size\":$SIZE}")
  local TOK=$(jq -r .token <<<"$INIT")
  curl -sS -X PUT "$B/api/upload/part/$TOK/0" -b "$1" -H "$2" --data-binary @"$3" > /dev/null
  curl -sS -X POST "$B/api/upload/complete/$TOK" -b "$1" -H "$2" > /dev/null
}
export -f upload_one
export B

echo "-- 10 轮:并发上传(30MB x2) + 打包下载 100 小文件 + 并发浏览"
for round in $(seq 1 10); do
  upload_one /tmp/m1.jar "$H1" $DATA/users/admin/big/big.bin "round$round.bin" &
  upload_one /tmp/m2.jar "$H2" $DATA/users/admin/big/big.bin "r$round.bin" &
  curl -sS -X POST $B/api/file/dlticket -b /tmp/m1.jar -H "$H1" -H 'Content-Type: application/json' \
    -d '{"share_id":1,"path":"big","names":["big.bin"]}' | jq -r .url > /tmp/zipurl
  curl -sS -o /dev/null "$B$(cat /tmp/zipurl)" &
  for i in 1 2 3 4 5; do curl -sS "$B/api/browse/1?path=" -b /tmp/m1.jar -H "$H1" > /dev/null & done
  wait
  echo "第 $round 轮后 RSS: $(rss) KB"
done

echo "-- 空闲 6 秒后(内存池回落)RSS:"
sleep 6
echo "$(rss) KB"
echo "-- 临时文件清理检查: $(ls /tmp/ndisk-memtest-r1/tmp 2>/dev/null | wc -l) 个残留"
echo "MEMTEST DONE"
