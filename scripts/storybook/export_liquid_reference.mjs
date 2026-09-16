// Generate independent golden vectors from an approved liquid prototype checkout.
// Usage: node scripts/storybook/export_liquid_reference.mjs REFERENCE_DIR OUTPUT
import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";
import crypto from "node:crypto";
const [directory, output] = process.argv.slice(2);
if (!directory || !output) throw new Error("Pass the reference directory and output JSON path");
const context = vm.createContext({ console, performance });
context.window = context;
const files = ["../squircle-path-kit-bundled.js", "../liquid-fusion-core.js", "ddr-gallery-core.js", "components12/fit-contour.js", "components12/sparse-contour.js"];
const sources = {};
for (const file of files) {
  const source = fs.readFileSync(path.resolve(directory, file), "utf8");
  sources[file] = crypto.createHash("sha256").update(source).digest("hex");
  vm.runInContext(source, context, { filename: file });
}
const pose = (x, y, w, h, r) => ({ cx: x + w / 2, cy: y + h / 2, w, h, r });
const cases = [
  { name: "connection-rebound", kind: "single", initial: pose(18, 18, 116, 32, 16), target: pose(18, 18, 402, 414, 32), anchor: [0, 0] },
  { name: "connection-reverse", kind: "single", initial: pose(18, 18, 116, 32, 16), target: pose(18, 18, 540, 338, 32), anchor: [0, 0], reverse: 22, seed: 42 },
  { name: "attachments", kind: "single", initial: pose(18, 18, 124, 32, 16), target: pose(18, 18, 320, 208, 20), anchor: [0, 0] },
  { name: "composer", kind: "single", initial: pose(18, 18, 420, 60, 30), target: pose(18, 18, 420, 180, 32), anchor: [0, 0] },
  { name: "history", kind: "single", initial: pose(18, 18, 360, 36, 12), target: pose(18, 18, 360, 206, 12), anchor: [0, 0], constraints: { left: true, right: true, top: true, bottom: false } },
  { name: "marker", kind: "single", initial: pose(25, 50, 2, 14, 1), target: pose(25, 185, 2, 14, 1), anchor: [0.5, 0.5] },
  { name: "member-fusion", kind: "pair", initial: pose(18, 18, 144, 32, 12), target: pose(18, 64, 320, 168, 20), anchor: [0, 0] },
  { name: "menu-fusion", kind: "pair", initial: pose(300, 18, 28, 28, 14), target: pose(138, 60, 190, 116, 20), anchor: [1, 0], reverse: 22 },
  { name: "comments", kind: "pair", initial: pose(18, 160, 90, 32, 16), target: pose(18, 206, 400, 242, 32), anchor: [0, 0], seed: 42 },
  { name: "split", kind: "split", initial: pose(90, 40, 180, 80, 40), targets: [pose(22, 130, 60, 60, 30), pose(232, 130, 90, 40, 20)], fractions: [0.3, 0.7], anchor: [0.5, 0.5] },
];
const D = context.DDRMaterial;
for (const item of cases) {
  const { initial, target } = item;
  const options = { seed: item.seed ?? 7321, anchorU: item.anchor[0], anchorV: item.anchor[1], capacity: Math.max(initial.w * initial.h, (target?.w ?? 0) * (target?.h ?? 0)), constraints: item.constraints || {} };
  item.options = options;
  const engine = item.kind === "pair" ? new D.Pair(initial, target, options) : item.kind === "split" ? new D.Multi(initial, item.targets, { ...options, fractions: item.fractions }) : new D.Motion(initial, options);
  item.actions = [
    { tick: 0, open: true },
    { tick: item.reverse ?? 120, open: false },
    { tick: 180, open: true },
    { tick: 240, open: false },
  ];
  item.frames = [];
  const captures = new Set([0, 1, 12, 24, 48, 96, 120, 144, 180, 240, 360, 720]);
  for (let tick = 0; tick <= 720; tick++) {
    for (const action of item.actions.filter((a) => a.tick === tick)) {
      if (item.kind === "single") engine.target(action.open ? target : initial);
      else engine.setOpen(action.open);
    }
    if (captures.has(tick)) {
      const sampler = context.DDRContourSparse.sampler(engine),
        body = engine.body.raw(),
        field = [];
      for (const [u, v] of [
        [-0.1, 0.5],
        [0, 0.5],
        [0.05, 0.05],
        [0.25, 0],
        [0.5, 0],
        [0.75, 0],
        [1, 0.5],
        [1.1, 0.5],
        [0.5, 1],
        [0.95, 0.95],
        [0.5, 0.5],
      ]) {
        const x = body.cx + (u - 0.5) * body.w,
          y = body.cy + (v - 0.5) * body.h;
        field.push([x, y, sampler(x, y)]);
      }
      item.frames.push({ tick, contour: context.DDRContourSparse.trace(engine).path, groups: engine.groups.map((g) => ({ pose: g.body.raw(), area: g.area, mass: g.mass, particles: g.p.map((p) => [p.id, p.x, p.y, p.qx, p.qy, p.vx, p.vy, p.m]) })), field });
    }
    engine.step(1 / 240);
  }
}
fs.mkdirSync(path.dirname(output), { recursive: true });
fs.writeFileSync(output, JSON.stringify({ version: "12.1", dt: 1 / 240, sources, cases }) + "\n");
console.log(`${cases.length} cases, ${cases.reduce((n, c) => n + c.frames.length, 0)} frames`);
