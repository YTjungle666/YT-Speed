// Web Worker：测速引擎跑在这里，主线程只收快照。
//
// 为什么必须在 Worker 里：同样的下载逻辑放主线程实测 6.64 Gbit/s，
// 挪到 Worker 是 7.23 Gbit/s（PLAN §0.5）。主线程还要渲染仪表盘，
// 让它去搬字节是双重浪费。

import init, { start, tick, abort, samples_json } from './pkg/speedtest_wasm.js';

let running = false;
let timer = null;

function stopLoop() {
  running = false;
  if (timer !== null) {
    clearTimeout(timer);
    timer = null;
  }
}

self.onmessage = async (ev) => {
  const msg = ev.data || {};
  if (msg.cmd === 'start') {
    stopLoop();
    await init();
    // 与 tick() 用同一个时钟（performance.now），否则时间基准不一致
    start(msg.base, JSON.stringify(msg.plan || {}), performance.now());
    running = true;

    let drainUntil = 0; // 结束后继续跑一小段，等上传结算到达

    const loop = () => {
      if (!running) return;
      const raw = tick(performance.now());
      if (raw !== 'null') {
        const frame = JSON.parse(raw);
        const finished = frame.phase === 'Done' || frame.phase === 'Aborted';
        if (finished) {
          // 关键：上传结算是“关连接→取 token”之后才回来的，
          // 阶段一结束就停表会让它永远落不了地（实测客户端估算比服务端高 6%）。
          if (drainUntil === 0 && frame.phase === 'Done') {
            drainUntil = performance.now() + 600;
          }
          if (performance.now() >= drainUntil) {
            stopLoop();
            self.postMessage({
              type: 'done',
              frame,
              samples: JSON.parse(samples_json()),
            });
            return;
          }
          timer = setTimeout(loop, 50);
          return;
        }
        self.postMessage({ type: 'frame', frame });
      }
      // 100 ms 心跳；core 的 200 ms 节拍由 wasm 内部节流
      timer = setTimeout(loop, 100);
    };
    loop();
  } else if (msg.cmd === 'abort') {
    abort(performance.now());
  }
};
