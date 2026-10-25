# 部署

生产位置：部署容器 `YT-Speed` / 192.0.2.20（Alpine 3.24，OpenRC）。

| 文件 | 用途 |
|---|---|
| `yt-speed.service` | systemd 单元（Debian/Ubuntu 主机） |
| `yt-speed.openrc` | OpenRC 服务脚本（Alpine 容器，当前使用） |
| `99-speedtestd-sysctl.conf` | 内核网络参数（10GbE 需要） |
| `Dockerfile` / `compose.yaml` | 容器化 |

## Alpine（当前部署）

    apk add openssh logrotate
    install -m755 yt-speed /usr/local/bin/yt-speed
    mkdir -p /srv/yt-speed/web        # 放 web/ 根下的 index.html + 哈希 js/wasm/css、pkg/、engine.worker.js
    install -m755 yt-speed.openrc /etc/init.d/yt-speed
    rc-update add yt-speed default
    rc-service yt-speed start

二进制必须用 **musl 静态目标**编译，Alpine 上没有 glibc：

    cargo build --release --target x86_64-unknown-linux-musl -p speedtest-server

日志 `/var/log/yt-speed.log`，由 `/etc/logrotate.d/yt-speed` 限 1 MB × 3 份。
