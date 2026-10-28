(function (root, factory) {
  if (typeof module === "object" && module.exports) {
    module.exports = factory();
  } else {
    root.RSBenchTC = factory();
  }
})(typeof self !== "undefined" ? self : this, function () {
  "use strict";

  var GRID_W = 64;
  var GRID_H = 36;
  var QUIET_UNITS = [2, 14, 12, 22];
  var DATA_X0 = 3;
  var DATA_Y0 = 15;
  var DATA_COLS = 8;
  var DATA_ROWS = 6;
  var WHITEN_MASK = 0xA5C35A3C96E1n;
  var PAYLOAD_MASK = (1n << 36n) - 1n;

  function crc8(bytes) {
    var crc = 0;
    for (var i = 0; i < bytes.length; i += 1) {
      crc ^= bytes[i] & 0xff;
      for (var bit = 0; bit < 8; bit += 1) {
        crc = (crc & 0x80) ? ((crc << 1) ^ 0x07) : (crc << 1);
        crc &= 0xff;
      }
    }
    return crc;
  }

  function encodeWord(type, payload) {
    var v40 = (BigInt(type) << 36n) | payload;
    var bytes = [
      Number((v40 >> 32n) & 0xffn),
      Number((v40 >> 24n) & 0xffn),
      Number((v40 >> 16n) & 0xffn),
      Number((v40 >> 8n) & 0xffn),
      Number(v40 & 0xffn)
    ];
    var word = (v40 << 8n) | BigInt(crc8(bytes));
    return word ^ WHITEN_MASK;
  }

  function encodeTime(unixMs) {
    return encodeWord(1, BigInt(Math.floor(unixMs)) & PAYLOAD_MASK);
  }

  function encodeCalib(w, h) {
    if (!Number.isInteger(w) || !Number.isInteger(h) || w < 1 || w > 262143 || h < 1 || h > 262143) {
      throw new RangeError("calibration dimensions must be integers in 1..262143");
    }
    return encodeWord(2, (BigInt(w) << 18n) | BigInt(h));
  }

  function encodeEnd() {
    return encodeWord(3, 0n);
  }

  function bits(whitened) {
    var result = [];
    for (var k = 0; k < 48; k += 1) {
      result.push(Number((whitened >> BigInt(47 - k)) & 1n));
    }
    return result;
  }

  function unitX(k, w) {
    return Math.floor(k * w / GRID_W + 0.5);
  }

  function unitY(k, h) {
    return Math.floor(k * h / GRID_H + 0.5);
  }

  function quietRect(w, h) {
    return {
      x0: unitX(QUIET_UNITS[0], w),
      y0: unitY(QUIET_UNITS[1], h),
      x1: unitX(QUIET_UNITS[2], w),
      y1: unitY(QUIET_UNITS[3], h)
    };
  }

  function cellRect(col, row, w, h) {
    return {
      x0: unitX(DATA_X0 + col, w),
      y0: unitY(DATA_Y0 + row, h),
      x1: unitX(DATA_X0 + col + 1, w),
      y1: unitY(DATA_Y0 + row + 1, h)
    };
  }

  function drawCode(ctx, whitened, w, h) {
    var quiet = quietRect(w, h);
    ctx.fillStyle = "#000";
    ctx.fillRect(quiet.x0, quiet.y0, quiet.x1 - quiet.x0, quiet.y1 - quiet.y0);

    var codeBits = bits(whitened);
    ctx.fillStyle = "#fff";
    for (var k = 0; k < codeBits.length; k += 1) {
      if (codeBits[k] !== 1) {
        continue;
      }
      var cell = cellRect(k % DATA_COLS, Math.floor(k / DATA_COLS), w, h);
      ctx.fillRect(cell.x0, cell.y0, cell.x1 - cell.x0, cell.y1 - cell.y0);
    }
  }

  return {
    GRID_W: GRID_W,
    GRID_H: GRID_H,
    QUIET_UNITS: QUIET_UNITS,
    DATA_X0: DATA_X0,
    DATA_Y0: DATA_Y0,
    DATA_COLS: DATA_COLS,
    DATA_ROWS: DATA_ROWS,
    WHITEN_MASK: WHITEN_MASK,
    PAYLOAD_MASK: PAYLOAD_MASK,
    crc8: crc8,
    encodeWord: encodeWord,
    encodeTime: encodeTime,
    encodeCalib: encodeCalib,
    encodeEnd: encodeEnd,
    bits: bits,
    unitX: unitX,
    unitY: unitY,
    quietRect: quietRect,
    cellRect: cellRect,
    drawCode: drawCode
  };
});
