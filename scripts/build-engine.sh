#!/bin/sh
# 构建测量引擎的 wasm 绑定到 web/pkg/。
#
# engine.worker.js 直接 `import init, { start, tick, ... } from './pkg/speedtest_wasm.js'`，
# 所以这个目录是 web/ 的运行时依赖，但它是 wasm-bindgen 的产物、不入库。
# 单独成脚本是因为它和 UI 走两套工具链：UI 用 trunk，引擎用 wasm-bindgen。
#
# 用法：scripts/build-engine.sh
set -eu

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

if ! command -v wasm-bindgen >/dev/null 2>&1; then
    echo "缺少 wasm-bindgen。装法：cargo install wasm-bindgen-cli --version <与 Cargo.lock 一致的版本>" >&2
    exit 1
fi

cd "$repo"
cargo build --release --target wasm32-unknown-unknown -p speedtest-wasm

rm -rf web/pkg
wasm-bindgen \
    --target web \
    --out-dir web/pkg \
    --out-name speedtest_wasm \
    target/wasm32-unknown-unknown/release/speedtest_wasm.wasm

ls -l web/pkg
