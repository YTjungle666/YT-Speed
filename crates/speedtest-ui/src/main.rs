//! CSR 入口：trunk 把这里编译成 wasm，`index.html` 里挂载。

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(speedtest_ui::App);
}
