// Opt-in, bounded diagnostics for the browser displaying this development fixture.
// Never read text inputs, story snapshots, URLs, or user files.
export function installClientTrace() {
  const id =
    crypto.randomUUID?.() ||
    Array.from(crypto.getRandomValues(new Uint8Array(16)), (n) => n.toString(16).padStart(2, "0"))
      .join("")
      .replace(/(.{8})(.{4})(.{4})(.{4})(.{12})/, "$1-$2-$3-$4-$5");
  const originalRaf = window.requestAnimationFrame;
  const restores = [];
  const rows = [];
  const longTasks = [];
  const inputs = [];
  const contexts = new Set();
  let backend = "pending",
    gpuInfo = null,
    readyAt = null,
    stopped = false;
  let submissions = 0,
    current = null,
    inputAt = null,
    lastTick = null,
    timer = null,
    serial = 0;
  let phase = "interaction",
    failure = null,
    artifact = null;
  const gpuProbe = new URLSearchParams(location.search).get("gpu") === "1";
  window.liquidTimestampLimit = 2400;
  const ticks = [];
  const round = (n) => Math.round(n * 100) / 100;
  const keep = (list, item, limit = 2400) => {
    list.push(item);
    if (list.length > limit) list.shift();
  };
  function wrap(object, key, make) {
    if (!object || typeof object[key] !== "function") return;
    const original = object[key],
      replacement = make(original);
    object[key] = replacement;
    restores.push(() => {
      if (object[key] === replacement) object[key] = original;
    });
  }
  function submitted(kind) {
    backend = kind;
    submissions++;
    if (current) current.draws++;
    if (inputAt !== null && readyAt !== null) {
      keep(inputs, round(performance.now() - inputAt), 100);
      inputAt = null;
    }
  }
  wrap(
    window,
    "requestAnimationFrame",
    (original) => (callback) =>
      original.call(window, (timestamp) => {
        const start = performance.now();
        if (readyAt === null || stopped) return callback(timestamp);
        const row = { id: ++serial, phase, at: round(start - readyAt), timestamp, cpu: 0, draws: 0, visible: document.visibilityState === "visible" };
        current = row;
        if (gpuProbe) window.liquidCurrentRaf = { id: row.id, at: row.at };
        try {
          callback(timestamp);
        } finally {
          current = null;
          if (gpuProbe) window.liquidCurrentRaf = undefined;
          row.cpu = round(performance.now() - start);
          if (row.draws) keep(rows, row);
        }
      }),
  );
  wrap(
    window.GPUQueue?.prototype,
    "submit",
    (original) =>
      function (...args) {
        const result = original.apply(this, args);
        submitted("webgpu");
        return result;
      },
  );
  wrap(
    window.GPUAdapter?.prototype,
    "requestDevice",
    (original) =>
      function (...args) {
        const info = this.info;
        if (info) gpuInfo = { vendor: info.vendor, architecture: info.architecture, device: info.device, description: info.description };
        return original.apply(this, args);
      },
  );
  for (const name of ["WebGLRenderingContext", "WebGL2RenderingContext"]) {
    for (const key of ["drawArrays", "drawElements", "drawArraysInstanced", "drawElementsInstanced"]) {
      wrap(
        window[name]?.prototype,
        key,
        (original) =>
          function (...args) {
            if (!contexts.has(this)) {
              contexts.add(this);
              gpuInfo = { renderer: this.getParameter(this.RENDERER), vendor: this.getParameter(this.VENDOR) };
            }
            const result = original.apply(this, args);
            submitted("webgl");
            return result;
          },
      );
    }
  }
  const start = () => {
    if (stopped || document.documentElement.dataset.ready !== "true") return;
    if (readyAt === null) {
      readyAt = performance.now();
      lastTick = null;
      badge.textContent = "正在记录 30 秒 · 请继续操作卡顿的控件";
      originalRaf.call(window, tick);
      timer = setInterval(flush, 5000);
      if (gpuProbe) window.liquidObserve = true;
    }
  };
  const input = () => {
    start();
    if (stopped || readyAt === null) return;
    inputAt = performance.now();
  };
  for (const name of ["pointerdown", "pointerup", "keydown"]) {
    window.addEventListener(name, input, { capture: true, passive: true });
    restores.push(() => window.removeEventListener(name, input, true));
  }
  let observer;
  if (window.PerformanceObserver?.supportedEntryTypes.includes("longtask")) {
    observer = new PerformanceObserver((list) => {
      for (const entry of list.getEntries())
        if (readyAt !== null && entry.startTime >= readyAt) {
          keep(longTasks, { at: round(entry.startTime - readyAt), duration: round(entry.duration) }, 100);
        }
    });
    observer.observe({ type: "longtask" });
  }
  const badge = document.createElement("div");
  badge.id = "client-trace";
  Object.assign(badge.style, { position: "fixed", right: "8px", bottom: "8px", zIndex: "99999", background: "#151719e8", color: "white", padding: "8px 12px", borderRadius: "8px", font: "12px/1.5 system-ui", pointerEvents: "none", whiteSpace: "pre-line" });
  badge.textContent = "点击展台控件后开始记录 30 秒";
  document.body.append(badge);
  function tick(timestamp) {
    if (stopped) return;
    if (readyAt !== null && document.visibilityState === "visible" && lastTick !== null) keep(ticks, round(timestamp - lastTick));
    lastTick = document.visibilityState === "visible" ? timestamp : null;
    originalRaf.call(window, tick);
  }
  function stats(values) {
    const sorted = values.filter(Number.isFinite).sort((a, b) => a - b);
    const at = (p) => (sorted.length ? sorted[Math.min(sorted.length - 1, Math.ceil(sorted.length * p) - 1)] : null);
    return { count: sorted.length, median: at(0.5), p95: at(0.95), p99: at(0.99), max: at(1) };
  }
  let saved = false;
  function report() {
    const rendered = rows.filter((row) => row.visible);
    // Intentional idle gaps are reported separately, not counted as missed frames.
    const intervals = rendered
      .slice(1)
      .map((row, i) => round(row.timestamp - rendered[i].timestamp))
      .filter((ms) => ms > 0 && ms < 250);
    const canvas = document.querySelector("canvas");
    const gpuFrames = new Map();
    for (const item of window.liquidTimestampRows || []) {
      const value = gpuFrames.get(item.frame) || { first: BigInt(item.startNs), last: BigInt(item.endNs), passes: 0 };
      value.first = value.first < BigInt(item.startNs) ? value.first : BigInt(item.startNs);
      value.last = value.last > BigInt(item.endNs) ? value.last : BigInt(item.endNs);
      value.passes += item.passes;
      gpuFrames.set(item.frame, value);
    }
    const gpuTime = (row) => {
      const frame = gpuFrames.get(row.id);
      return frame ? Number(frame.last - frame.first) / 1e6 : NaN;
    };
    const phases = {};
    for (const name of new Set(rendered.map((row) => row.phase))) {
      const part = rendered.filter((row) => row.phase === name);
      phases[name] = { cpuMs: stats(part.map((row) => row.cpu)), gpuRenderMs: stats(part.map(gpuTime)), gpuPasses: stats(part.map((row) => gpuFrames.get(row.id)?.passes)), frames: part.length };
    }
    return {
      schema: 1,
      id,
      artifact,
      complete: stopped,
      failure,
      gpuProbe,
      phase,
      phases,
      seconds: readyAt === null ? 0 : round((performance.now() - readyAt) / 1000),
      backend,
      gpuInfo,
      environment: { userAgent: navigator.userAgent, secureContext: isSecureContext, webgpuAvailable: !!navigator.gpu, viewport: [innerWidth, innerHeight], dpr: devicePixelRatio, canvas: canvas ? [canvas.width, canvas.height] : null, visibility: document.visibilityState, focused: document.hasFocus() },
      frameCpuMs: stats(rendered.map((row) => row.cpu)),
      renderIntervalMs: stats(intervals),
      browserIntervalMs: stats(ticks),
      inputToSubmitMs: stats(inputs),
      submissions,
      longTasks,
      gpuRenderMs: stats(rendered.map(gpuTime)),
      gpuPasses: stats(rendered.map((row) => gpuFrames.get(row.id)?.passes)),
      gpuErrors: (window.liquidTimestampErrors || []).slice(0, 20),
      discardedPathPasses: window.liquidPathDiscarded || 0,
      frames: rendered.slice(-180).map(({ at, cpu, draws }) => ({ at, cpu, draws })),
      scope: "Visible browser rAF cadence and CPU submission; not physical presentation. Optional GPU timestamps cover render passes; their query/readback overhead affects CPU. Idle gaps >=250ms excluded. No application text collected.",
    };
  }
  async function flush() {
    if (readyAt !== null && performance.now() - readyAt >= 30000) {
      stopped = true;
      clearInterval(timer);
      restores.reverse().forEach((restore) => restore());
      observer?.disconnect();
      if (gpuProbe) await window.liquidTimestampCleanup?.();
      window.liquidPathStoreCleanup?.();
    }
    const data = report();
    badge.textContent = `${stopped ? "采样结束" : readyAt === null ? "点击展台控件后开始记录 30 秒" : "正在记录 · 请操作卡顿的控件"} · ${backend}\n浏览器间隔 ${data.browserIntervalMs.median ?? "—"} ms · 绘制间隔 P95 ${data.renderIntervalMs.p95 ?? "—"} ms\nCPU P95 ${data.frameCpuMs.p95 ?? "—"} ms · ${saved ? "数据已回传" : "等待回传"}`;
    badge.dataset.report = JSON.stringify(data);
    try {
      const response = await fetch("./__client_trace", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(data), cache: "no-store" });
      saved = response.ok;
    } catch {
      saved = false;
    }
    if (stopped) badge.textContent += saved ? "\n最终数据已回传" : "\n回传失败";
  }
  return {
    start,
    artifact: (value) => {
      artifact = value;
    },
    phase: (value) => {
      phase = value;
    },
    fail: (error) => {
      failure = String(error);
      badge.textContent = failure;
    },
  };
}
