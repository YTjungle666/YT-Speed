# 多阶段构建：产物镜像只含一个静态二进制 + 静态资产。
# 用 debian 而不是 alpine 作为运行层：musl 的 malloc 在 10GbE 下碎片更多，
# 而且基线数字都是在 glibc 上取的（PLAN §4.2），换 libc 等于换基线。

FROM rust:1-slim AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY web ./web
RUN cargo build --release --bin speedtestd \
    && strip target/release/speedtestd

FROM debian:bookworm-slim
# curl 只用于 HEALTHCHECK（镜像自带探针比依赖外部编排更可靠）
RUN apt-get update \
    && apt-get install -y --no-install-recommends curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --home /srv/speedtest --shell /usr/sbin/nologin speedtest
WORKDIR /srv/speedtest
COPY --from=build /src/target/release/speedtestd /usr/local/bin/speedtestd
COPY --from=build /src/web ./web
USER speedtest
EXPOSE 8080
ENV RUST_LOG=info
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -fsS http://127.0.0.1:8080/healthz || exit 1
ENTRYPOINT ["/usr/local/bin/speedtestd"]
CMD ["--listen", "0.0.0.0:8080", "--web-dir", "/srv/speedtest/web"]
