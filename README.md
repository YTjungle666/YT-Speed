# YT-Speed

家庭内网测速服务（Leptos/WASM 前端 + Rust 测量引擎 + 原生命令行），
设计取向上参考 LibreSpeed 但为全栈 Rust 重写。设计与验收标准见
`~/Plan/yt-speed/PLAN.md`（本仓不放计划；部署见 `deploy/`）。

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

构建并部署 Leptos 页面到 `web/app/`：

```sh
sudo apt install binaryen   # 提供 wasm-opt，仅构建时需要
./scripts/build-ui.sh
```

在仓库根目录启动静态服务后打开 `http://127.0.0.1:8092/app/`：

```sh
cd web
python3 -m http.server 8092
```

构建使用 `/app/` 作为 Trunk `public_url`。P4b 浏览器矩阵：

| 浏览器 | 状态 |
|---|---|
| Chromium 156 | 通过：桌面与 390×844 渲染、无横向溢出、设置输入、阶段配色、历史 25→20 截断、复制成功/降级、无控制台错误 |
| Firefox | 未验证：本机未安装 Firefox |
| WebKit | 未验证 |
