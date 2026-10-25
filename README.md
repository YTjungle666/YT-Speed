# YT-Speed

[![ci](https://github.com/YTjungle666/YT-Speed/actions/workflows/ci.yml/badge.svg)](https://github.com/YTjungle666/YT-Speed/actions/workflows/ci.yml)

家庭内网测速服务（Leptos/WASM 前端 + Rust 测量引擎 + 原生命令行），
设计取向上参考 LibreSpeed 但为全栈 Rust 重写。

## 资源上限（硬约束）

服务会长年跑在局域网里，被浏览器、CLI、脚本反复打，所以**任何一个对外接口都不允许
有随输入无限增长的结构**。上限集中在 `crates/speedtest-server/src/limits.rs`，每条都
写了“为什么是这个数”：

| 结构 | 上限 | 超过时 |
|---|---|---|
| WS 单条消息 | 8 MiB（客户端 1 MiB 帧的 8 倍余量） | 连接以 1009 关闭，RSS 不涨 |
| WS 并发连接 | 64 | 新连接直接 503，不排队 |
| 结算表条目 | 1024（10 s 过期） | 挤掉最旧的一条 |
| 静态资产 | 64 MiB | 413；资产整体**流式发送**，不读进内存 |
| 下载响应 | 每次 ≤1024 MiB，流式 + 内核背压 | — |
| 上传 | 逐帧丢弃，只累加长度 | — |

实测（回环）：16 MiB 帧被拒时服务端 RSS 保持 12.4 MiB、`/healthz` 仍 200；
70 路并发 WS 握手得到 64×101 + 6×503。**整个服务不写任何磁盘文件**（没有结果库、
没有临时文件），日志只走 stdout 交给 journald。

`core` 与后续 `transport` 里的采样缓冲都是**定容环形缓冲**，不随测试时长增长。

## 结构

| crate | 作用 |
|---|---|
| `speedtest-core` | 纯逻辑、无 async、无 IO：命令/事件状态机、ramp/grace、带宽换算、ping/jitter、取消 |
| `speedtest-transport` | (P3) wasm 侧 fetch 流式下载 / WebSocket 上传；native 侧 hyper |
| `speedtest-server` | (P2) axum 服务端 |
| `speedtest-cli` | (P6) CLI |

core 的时间一律由调用方传入（`apply(ev, now_ms)`），所以测试不需要 sleep，
也不是“假时钟特例”——生产路径与测试路径是同一份代码。

换算照抄上游 `speedtest_worker.js`（v6.3.1）：
`speed = bytes / secs`、`mbps = speed * 8 * 1.06 / (use_mebibits ? 1048576 : 1000000)`、
下载 grace 1.5 s、上传 grace 3 s、`time_auto` 时每 tick `bonus += min(5*speed/100000, 400)`。

## Web UI

界面就挂在站点根（`http://host/`），构建走两套工具链：

```sh
sudo apt install binaryen          # 提供 wasm-opt，trunk 需要
cargo install wasm-bindgen-cli      # 版本要和 Cargo.lock 里的 wasm-bindgen 一致
./scripts/build-engine.sh           # 测量引擎 → web/pkg/
./scripts/build-ui.sh               # Leptos 页面 → web/ 根（index.html + 哈希资源）
```

本地预览：

```sh
cd web
python3 -m http.server 8092   # 打开 http://127.0.0.1:8092/
```

Trunk 的 `public_url` 是 `/`，资源都是根路径；正式环境的 wasm MIME 由 `yt-speed` 自己发。

## 开发与验证工具

这两个是验收用的，不是产品路径：

| 工具 | 用途 |
|---|---|
| `scripts/verify-server.sh [host:port]` | 逐端点核对头/体、上传三种形态、熵、限速 |
| `cargo run --release --example wsclient -- ws://host/backend/ws-upload 10 [frame_kib]` | 上传压测：推指定 GB 的 1 MiB 帧，和服务端结算对照 |

P4b 浏览器矩阵：

| 浏览器 | 状态 |
|---|---|
| Chromium 156 | 通过：桌面与 390×844 渲染、无横向溢出、设置输入、阶段配色、历史 25→20 截断、复制成功/降级、无控制台错误 |
| Firefox | 未验证：本机未安装 Firefox |
| WebKit | 未验证 |

## 许可

PolyForm Noncommercial License 1.0.0 —— 个人非商业使用免费，商业用途需另行授权。
全文见 [LICENSE](LICENSE)。
