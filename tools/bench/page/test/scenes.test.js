const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const scenes = require("../scenes.js");

const vectors = JSON.parse(fs.readFileSync(path.join(__dirname, "../../vectors/static_scene.json"), "utf8"));

test("xorshift32 vector", () => {
  const next = scenes.xorshift32(0x9e3779b9);
  assert.deepEqual([next(), next(), next()], vectors.xorshift32_first3);
});

test("static rectangle vectors", () => {
  for (const item of vectors.sizes) {
    const rects = scenes.staticRects(item.width, item.height);
    assert.equal(rects.length, 160);
    assert.deepEqual(rects.slice(0, 4), item.first4);
    assert.deepEqual(rects.at(-1), item.last);
    assert.equal(rects.flat().reduce((sum, value) => (sum + value) >>> 0, 0), item.checksum);
  }
});

test("parseParams validates and applies last values", () => {
  assert.deepEqual(scenes.parseParams("#scene=static"), { ok: true, scene: "static", seconds: 60, calib: 4, delay: 0 });
  assert.deepEqual(scenes.parseParams("scene=drag&seconds=5&calib=2&delay=1.5"), { ok: true, scene: "drag", seconds: 5, calib: 2, delay: 1.5 });
  for (const hash of ["seconds=5", "scene=foo", "scene=static&seconds=0", "scene=static&seconds=3601", "scene=static&seconds=1.5", "scene=static&calib=0.5", "scene=static&delay=31"]) {
    assert.equal(scenes.parseParams(hash).ok, false, hash);
  }
  assert.equal(scenes.parseParams("#scene=static&scene=video").scene, "video");
});

test("phaseAt follows the page timeline", () => {
  const p = { scene: "static", seconds: 60, calib: 4, delay: 0 };
  assert.deepEqual(scenes.phaseAt(0, p), { phase: "calib", white: true });
  assert.deepEqual(scenes.phaseAt(0.49, p), { phase: "calib", white: true });
  assert.deepEqual(scenes.phaseAt(0.5, p), { phase: "calib", white: false });
  assert.deepEqual(scenes.phaseAt(3.99, p), { phase: "calib", white: false });
  assert.deepEqual(scenes.phaseAt(4, p), { phase: "scene", sceneT: 0 });
  assert.equal(scenes.phaseAt(63.99, p).phase, "scene");
  assert.deepEqual(scenes.phaseAt(64, p), { phase: "end" });
  assert.deepEqual(scenes.phaseAt(1.9, { ...p, delay: 2 }), { phase: "delay" });
  assert.equal(scenes.phaseAt(1000, { ...p, scene: "calib" }).phase, "calib");
});

function fakeContext() {
  const calls = [];
  const ctx = {
    calls,
    fillRect(...args) { calls.push(["fillRect", ...args]); },
    drawImage(...args) { calls.push(["drawImage", ...args]); },
    putImageData(...args) { calls.push(["putImageData", ...args]); },
    createImageData(w, h) { return { data: new Uint8ClampedArray(w * h * 4), width: w, height: h }; },
    beginPath() { calls.push(["beginPath"]); },
    arc(...args) { calls.push(["arc", ...args]); },
    fill() { calls.push(["fill"]); },
  };
  return ctx;
}

test("scene drawing functions are deterministic and use the background first", () => {
  const staticCtx = fakeContext();
  scenes.drawStatic(staticCtx, 320, 200);
  assert.equal(staticCtx.calls.filter((call) => call[0] === "fillRect").length, 161);

  for (const t of [0, 3.7]) {
    const a = fakeContext();
    const b = fakeContext();
    scenes.drawScroll(a, 640, 360, t);
    scenes.drawScroll(b, 640, 360, t);
    assert.deepEqual(a.calls, b.calls);
  }

  const dragCtx = fakeContext();
  scenes.drawDrag(dragCtx, 640, 360, 0, {});
  assert.equal(dragCtx.calls[0][0], "drawImage");
});

test("drawVideo accepts an injected offscreen canvas factory", () => {
  const offCtx = fakeContext();
  const fakeCanvas = { getContext: () => offCtx };
  const ctx = fakeContext();
  scenes.drawVideo(ctx, 640, 360, 1.2, (w, h) => {
    assert.equal(w, 240);
    assert.equal(h, 135);
    return fakeCanvas;
  });
  assert.equal(offCtx.calls[0][0], "putImageData");
  assert.equal(ctx.calls[0][0], "drawImage");
});
