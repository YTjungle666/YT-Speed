# web/

生产页面全部构建产出，服务端只读这里：

| 路径 | 内容 |
|---|---|
| `app/` | Leptos UI（`scripts/build-ui.sh` 生成，`index.html` + 哈希命名的 js/wasm/css） |
| `pkg/` | 测量引擎的 wasm 绑定，被 `engine.worker.js` 以 `../pkg/` 导入 |
| `engine.worker.js` | 测量 worker（Web Worker + module 类型） |

`/` 会 302 到 `/app/index.html`。以前的 P3 验证页、legacy 对照页、路数扫描页都是
开发期的脚手架，已删除，不再随部署发布。
