# web/

站点根就是界面：`http://host/` 直接是 YT-Speed，没有 `/app/` 这类前缀。

| 路径 | 内容 |
|---|---|
| `index.html` | 界面入口（`scripts/build-ui.sh` 生成，资源用根路径绝对地址） |
| `speedtest-ui-*.js` / `*_bg.wasm` / `styles-*.css` | trunk 构建产物，带内容哈希，长缓存 |
| `pkg/` | 测量引擎的 wasm 绑定，被 `engine.worker.js` 以 `../pkg/` 导入 |
| `engine.worker.js` | 测量 worker（Web Worker + module 类型） |

前端路由（`/settings` 这类无扩展名路径）由服务端回 `index.html`，交给客户端路由。
