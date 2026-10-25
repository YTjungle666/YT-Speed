// Web Worker：测速引擎跑在这里，主线程只收快照。
//
// 为什么必须在 Worker 里：同样的下载逻辑放主线程实测 6.64 Gbit/s，
// 挪到 Worker 是 7.23 Gbit/s（PLAN §0.5）。主线程还要渲染仪表盘，
// 让它去搬字节是双重浪费。

import init, { start, tick, abort, samples_json, last_error } from './pkg/speedtest_wasm.js';

let running = false;
let timer = null;
let inited = false;

// worker 里的异常默认是静默的：页面只看到“没有任何响应”，控制台也是干净的。
// 排查过一次“worker 完全不回消息”，就是靠这几行才看到真实原因。
self.onerror = (e) =>
  self.postMessage({ type: 'error', where: 'worker', message: String(e.message || e) });
self.addEventListener('unhandledrejection', (e) =>
  self.postMessage({
    type: 'error',
    where: 'rejection',
    message: String((e.reason && (e.reason.message || e.reason)) || 'unknown'),
  }));

function stopLoop() {
  running = false;
  if (timer !== null) {
    clearTimeout(timer);
    timer = null;
  }
}

async function ensureInit() {
  if (inited) return;
  await init();
  inited = true;
}

self.onmessage = async (ev) => {
  const msg = ev.data || {};

  // 预热：页面在跑任何测试之前先让 wasm 落地。
  // 否则第一次测试要和遗留连接抢“每主机 6 个连接”的名额，init 会一直挂着。
  if (msg.cmd === 'init') {
    try {
      await ensureInit();
      self.postMessage({ type: 'init-done' });
    } catch (err) {
      self.postMessage({ type: 'error', where: 'init', message: String((err && err.message) || err) });
    }
    return;
  }

  if (msg.cmd === 'start') {
    stopLoop();
    try {
      await ensureInit();
      // 与 tick() 用同一个时钟（performance.now），否则时间基准不一致
      start(msg.base, JSON.stringify(msg.plan || {}), performance.now());
    } catch (err) {
      self.postMessage({ type: 'error', where: 'start', message: String((err && err.message) || err) });
      return;
    }
    running = true;

    // 看门狗：手机锁屏/切后台时 setTimeout 会被节流到分钟级，tick 不再推进，
    // 引擎就永远走不到 Done（实测服务端会一直以 400 Mbit/s 往外写）。
    // 这里用绝对截止时间兜底，节流后第一拍就会命中。
    const plan = msg.plan || {};
    const budgetMs = (Number(plan.dl_seconds) || 15) * 1000 + (Number(plan.ul_seconds) || 15) * 1000 + 30000;
    const deadline = performance.now() + budgetMs;

    let drainUntil = 0; // 结束后继续跑一小段，等上传结算到达

    let lastFrame = null;
    let lastPhase = '';
    // 阶段打点：把「客户端认为自己进入/离开某阶段」的时间发回服务端，
    // 才能判断残留流量是客户端没收手，还是服务端写超了。
    const mark = (ev) => {
      try {
        fetch(`${msg.base || ''}/backend/mark?ev=${ev}&t=${Math.round(performance.now())}`, { keepalive: true });
      } catch (_) {}
    };
    const CLIENT_VER = 6;
    mark('start&v=' + CLIENT_VER);

    const loop = () => {
      if (!running) return;
      if (performance.now() > deadline) {
        // 先让引擎走中止流程（会下发 StopAll 关掉所有流），再报结果
        abort(performance.now());
        const aborted = tick(performance.now());
        if (aborted !== 'null') lastFrame = JSON.parse(aborted);
        stopLoop();
        self.postMessage({
          type: 'done',
          frame: lastFrame || { phase: 'Aborted', failed: true },
          samples: JSON.parse(samples_json()),
          error: '看门狗超时：页面可能被切到后台，已中止',
        });
        return;
      }
      const raw = tick(performance.now());
      if (raw !== 'null') {
        const frame = JSON.parse(raw);
        if (frame.phase !== lastPhase) {
          lastPhase = frame.phase;
          mark(frame.phase);
        }
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
              // failed 时带上原因：远程机器上没法开 devtools 看 console
              error: frame.failed ? last_error() || '（未捕获到具体原因）' : null,
            });
            return;
          }
          timer = setTimeout(loop, 50);
          return;
        }
        lastFrame = frame;
        self.postMessage({ type: 'frame', frame });
      }
      // 100 ms 心跳；core 的 200 ms 节拍由 wasm 内部节流
      timer = setTimeout(loop, 100);
    };
    loop();
  } else if (msg.cmd === 'abort') {
    abort(performance.now());
  } else if (msg.cmd === 'stop') {
    // 页面隐藏/关闭时的兜底：主线程发了 stop 就立刻关流，别等引擎自己走到 Done
    abort(performance.now());
    stopLoop();
  }
};
