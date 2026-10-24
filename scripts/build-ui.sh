#!/bin/sh
# 构建 Leptos UI 并部署到 web/app/。
#
# 为什么部署到 web/app/ 而不是 web/ 根：web/ 根下还有验证页（index.html）、
# legacy 对照页、engine.worker.js 和 speedtest_wasm 的 pkg/。trunk 会按自己的
# 规则覆盖 dist，把产物放进子目录最省事，切主站的时候再翻。
#
# 用法：scripts/build-ui.sh
set -eu

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo/crates/speedtest-ui"

if ! command -v trunk >/dev/null 2>&1; then
    echo "缺少 trunk。装法：https://trunkrs.dev（本机已放在 /usr/local/bin/trunk）" >&2
    exit 1
fi

# 只清自己生成的文件，避免误删别的页面
trunk build --release
mkdir -p "$repo/web/app"
find "$repo/web/app" -maxdepth 1 -type f -name 'speedtest-ui-*' -delete
find "$repo/web/app" -maxdepth 1 -type f -name 'styles-*.css' -delete
cp dist/index.html dist/*.js dist/*.wasm dist/*.css "$repo/web/app/"

size=$(find "$repo/web/app" -maxdepth 1 -name "*_bg.wasm" -printf "%s\n" | head -1)
echo "已部署到 web/app/（wasm ${size} 字节）"
