#!/bin/sh
# P2 验收脚本：逐端点核对头/体、上传三种形态、熵、限速。
# 用法：./scripts/verify-server.sh [host:port]（默认 127.0.0.1:8080）
set -u
HOST="${1:-127.0.0.1:8080}"
BASE="http://$HOST"
fail=0
ok() { printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { printf '  \033[31m✗\033[0m %s\n' "$1"; fail=$((fail + 1)); }

echo "== 探针"
curl -fsS "$BASE/healthz" >/dev/null && ok "healthz 200" || bad "healthz"

echo "== 下载头"
hdr=$(curl -sSI "$BASE/backend/garbage?ckSize=8" | tr -d '\r')
echo "$hdr" | grep -qi '^content-length: 8388608$' && ok "content-length=8388608" || bad "content-length 不对"
echo "$hdr" | grep -qi '^content-type: application/octet-stream' && ok "content-type" || bad "content-type"
echo "$hdr" | grep -qi '^cache-control: .*no-store' && ok "no-store" || bad "cache-control"

echo "== ckSize 边界"
len() { curl -sSI "$BASE/backend/garbage?ckSize=$1" | tr -d '\r' | awk 'tolower($1)=="content-length:"{print $2}'; }
[ "$(len abc)" = "4194304" ] && ok "非法值 → 4 MiB" || bad "非法值应给 4 MiB，实得 $(len abc)"
[ "$(len 5000)" = "1073741824" ] && ok ">1024 → 1024 MiB" || bad "上界截断不对"

echo "== empty"
curl -sSI "$BASE/backend/empty" | tr -d '\r' | grep -qi '^content-length: 0' && ok "GET 零字节" || bad "GET 不是零字节"
conns=$(curl -s -o /dev/null -w '%{num_connects} ' "$BASE/backend/empty" "$BASE/backend/empty")
[ "$conns" = "1 0 " ] && ok "同连接复用（第二次 num_connects=0）" || bad "连接没有复用：$conns"

echo "== 上传三种形态（各 8 MiB）"
tmp=$(mktemp)
head -c 8388608 /dev/urandom >"$tmp"
for shape in "content-length:--data-binary @$tmp" "chunked:-H Transfer-Encoding:chunked --data-binary @-" "100-continue:-H Expect:100-continue --data-binary @$tmp"; do
  name=${shape%%:*}
  args=$(echo "$shape" | cut -d: -f2-)
  if [ "$name" = "chunked" ]; then
    got=$(cat "$tmp" | curl -s -o /dev/null -D- -H 'Transfer-Encoding: chunked' --data-binary @- "$BASE/backend/empty" | tr -d '\r' | awk 'tolower($1)=="x-received-bytes:"{print $2}')
  else
    got=$(eval curl -s -o /dev/null -D- $args "$BASE/backend/empty" | tr -d '\r' | awk 'tolower($1)=="x-received-bytes:"{print $2}')
  fi
  [ "$got" = "8388608" ] && ok "$name 收到 8388608" || bad "$name 收到 $got"
done
rm -f "$tmp"

echo "== 随机性"
curl -s "$BASE/backend/garbage?ckSize=1" -o /tmp/.entropy.bin
ent=$(python3 - <<'PY'
import collections, math
d = open('/tmp/.entropy.bin','rb').read()
n = len(d); c = collections.Counter(d)
print(f"{-sum((v/n)*math.log2(v/n) for v in c.values()):.4f}")
PY
)
awk -v e="$ent" 'BEGIN{ if (e+0 > 7.9) print "  \033[32m✓\033[0m 熵 " e " bit/byte"; else { print "  \033[31m✗\033[0m 熵 " e; exit 1 } }' || fail=$((fail + 1))
rm -f /tmp/.entropy.bin

echo "== getIP"
curl -s "$BASE/backend/getIP" | grep -q '"processedString"' && ok "字段名与上游一致" || bad "getIP 字段缺失"

echo
if [ "$fail" -eq 0 ]; then echo "全部通过"; else echo "$fail 项失败"; fi
exit "$fail"
