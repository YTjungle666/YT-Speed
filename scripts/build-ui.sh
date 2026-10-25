#!/bin/sh
# 构建 Leptos UI 并部署到 web/ 根。
#
# 界面地址就是站点根（http://host/），所以 index.html、哈希命名的 js/wasm/css
# 都放在 web/ 根下；engine.worker.js 和 pkg/ 本来就在根，互不冲突。
# 开发期的验证页、legacy 对照页已经删除，根目录不再有多套入口。
#
# 用法：scripts/build-ui.sh
set -eu

# Trunk 0.21 把 NO_COLOR 的值当成 --no-color 的布尔参数；常见的 `NO_COLOR=1`
# 会在真正构建前报错。脚本不依赖颜色输出，构建期间解除这个变量。
unset NO_COLOR

# P4b 的视图组件会把未优化 wasm 推高约 450 KiB。先按体积优化 Rust，再由 Trunk
# 以 `wasm-opt -Oz` 收敛；这组参数让相对 P4a 的增量保持在 60 KiB 以内。
export CARGO_PROFILE_RELEASE_OPT_LEVEL=s
export CARGO_PROFILE_RELEASE_LTO=fat
export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo/crates/speedtest-ui"

if ! command -v trunk >/dev/null 2>&1; then
    echo "缺少 trunk。装法：https://trunkrs.dev（本机已放在 /usr/local/bin/trunk）" >&2
    exit 1
fi
if ! command -v wasm-opt >/dev/null 2>&1; then
    echo "缺少 wasm-opt。Debian/Ubuntu：sudo apt install binaryen" >&2
    exit 1
fi

# 只清自己生成的文件，避免误删别的页面
trunk build --release
# 只清自己生成的带哈希文件，别误删 engine.worker.js / pkg/ / README
find "$repo/web" -maxdepth 1 -type f -name 'speedtest-ui-*' -delete
find "$repo/web" -maxdepth 1 -type f -name 'styles-*.css' -delete
cp dist/index.html dist/*.js dist/*.wasm dist/*.css "$repo/web/"

size=$(find "$repo/web" -maxdepth 1 -name "*_bg.wasm" -printf "%s\n" | head -1)
echo "已部署到 web/（wasm ${size} 字节）"
