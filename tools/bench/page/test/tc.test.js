const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const tc = require("../tc.js");

const vectors = JSON.parse(fs.readFileSync(path.join(__dirname, "../../vectors/timecode.json"), "utf8"));

test("CRC-8 check vector", () => {
  assert.equal(tc.crc8([...Buffer.from("123456789")]), 0xf4);
});

test("timecode vectors and bits", () => {
  for (const item of vectors.cases) {
    let code;
    if (item.code.type === "time") {
      code = tc.encodeTime(item.code.ms);
    } else if (item.code.type === "calib") {
      code = tc.encodeCalib(item.code.width, item.code.height);
    } else {
      code = tc.encodeEnd();
    }
    assert.equal(code.toString(16).padStart(12, "0"), item.whitened);
    assert.equal(tc.bits(code).join(""), item.bits);
  }
  const sample = vectors.cases.find((item) => item.name === "time_sample");
  assert.equal(tc.encodeTime(1792507200123).toString(16).padStart(12, "0"), sample.whitened);
  assert.equal(tc.encodeTime(2 ** 36 + 7), tc.encodeTime(7));
});

test("timecode geometry", () => {
  for (const item of vectors.geometry) {
    assert.deepEqual(Object.values(tc.quietRect(item.width, item.height)), item.quiet);
    assert.deepEqual(Object.values(tc.cellRect(0, 0, item.width, item.height)), item.cell_0_0);
    assert.deepEqual(Object.values(tc.cellRect(7, 5, item.width, item.height)), item.cell_7_5);
  }
});

test("calibration dimensions are checked", () => {
  assert.throws(() => tc.encodeCalib(0, 5), RangeError);
  assert.throws(() => tc.encodeCalib(262144, 5), RangeError);
  assert.throws(() => tc.encodeCalib(1.5, 5), RangeError);
});

test("drawCode draws quiet area and one rectangle per set bit", () => {
  const calls = [];
  const ctx = {};
  Object.defineProperty(ctx, "fillStyle", {
    set(value) { calls.push(["style", value]); },
  });
  ctx.fillRect = (...args) => calls.push(["rect", ...args]);
  const sample = vectors.cases.find((item) => item.name === "time_sample");
  const word = tc.encodeTime(sample.code.ms);
  tc.drawCode(ctx, word, 1920, 1080);
  assert.deepEqual(calls[0], ["style", "#000"]);
  assert.deepEqual(calls[1], ["rect", 60, 420, 300, 240]);
  assert.equal(calls[2][0], "style");
  assert.equal(calls[2][1], "#fff");
  const white = tc.bits(word).filter(Boolean).length;
  assert.equal(calls.filter((call) => call[0] === "rect").length, white + 1);
  const firstBit = tc.bits(word).findIndex(Boolean);
  const first = tc.cellRect(firstBit % 8, Math.floor(firstBit / 8), 1920, 1080);
  assert.deepEqual(calls[3], ["rect", first.x0, first.y0, first.x1 - first.x0, first.y1 - first.y0]);
});
