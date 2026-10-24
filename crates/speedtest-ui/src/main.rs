//! CSR 入口：trunk 把这里编译成 wasm，`index.html` 里挂载。
//!
//! 只在 wasm32 下有意义；host 目标保留一个空 main，是为了 `cargo test` / `cargo clippy`
//! 在 workspace 根目录能过（否则报 `console_error_panic_hook` 找不到——它被限制在
//! wasm32 依赖里）。

#[cfg(target_arch = "wasm32")]
fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(speedtest_ui::App);
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!("speedtest-ui 只构建 wasm32：用 scripts/build-ui.sh（trunk）构建");
}
