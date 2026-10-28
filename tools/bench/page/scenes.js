(function (root, factory) {
  if (typeof module === "object" && module.exports) {
    module.exports = factory();
  } else {
    root.RSBenchScenes = factory();
  }
})(typeof self !== "undefined" ? self : this, function () {
  "use strict";

  var SCENES = ["static", "scroll", "video", "drag", "calib"];

  function xorshift32(seed) {
    var s = seed >>> 0;
    return function next() {
      s = (s ^ (s << 13)) >>> 0;
      s = (s ^ (s >>> 17)) >>> 0;
      s = (s ^ (s << 5)) >>> 0;
      return s;
    };
  }

  function staticRects(w, h) {
    var next = xorshift32(0x9E3779B9);
    var maxW = Math.max(1, Math.floor(w / 4));
    var maxH = Math.max(1, Math.floor(h / 6));
    var rects = [];
    for (var i = 0; i < 160; i += 1) {
      var x = next() % w;
      var y = next() % h;
      var rw = 16 + next() % maxW;
      var rh = 8 + next() % maxH;
      var g = next() % 256;
      rects.push([x, y, rw, rh, g]);
    }
    return rects;
  }

  function decodePart(value) {
    try {
      return decodeURIComponent(value.replace(/\+/g, " "));
    } catch (error) {
      return value;
    }
  }

  function parseParams(hash) {
    var text = hash == null ? "" : String(hash);
    if (text.charAt(0) === "#") {
      text = text.slice(1);
    }
    if (text.charAt(0) === "?") {
      text = text.slice(1);
    }
    var values = Object.create(null);
    if (text !== "") {
      var pairs = text.split("&");
      for (var i = 0; i < pairs.length; i += 1) {
        if (pairs[i] === "") {
          continue;
        }
        var equal = pairs[i].indexOf("=");
        var rawKey = equal < 0 ? pairs[i] : pairs[i].slice(0, equal);
        var rawValue = equal < 0 ? "" : pairs[i].slice(equal + 1);
        var key = decodePart(rawKey);
        if (key !== "") {
          values[key] = decodePart(rawValue);
        }
      }
    }

    if (!Object.prototype.hasOwnProperty.call(values, "scene") || values.scene === "") {
      return { ok: false, error: "scene is required" };
    }
    var scene = values.scene;
    if (SCENES.indexOf(scene) < 0) {
      return { ok: false, error: "unknown scene: " + scene };
    }

    function numberValue(name, defaultValue, min, max, integer) {
      if (!Object.prototype.hasOwnProperty.call(values, name)) {
        return { value: defaultValue };
      }
      var raw = values[name].trim();
      var number = Number(raw);
      if (raw === "" || !Number.isFinite(number) || (integer && !Number.isInteger(number)) || number < min || number > max) {
        return { error: name + " must be " + (integer ? "an integer " : "a number ") + "in " + min + ".." + max };
      }
      return { value: number };
    }

    var seconds = numberValue("seconds", 60, 1, 3600, true);
    if (seconds.error) {
      return { ok: false, error: seconds.error };
    }
    var calib = numberValue("calib", 4, 1, 30, false);
    if (calib.error) {
      return { ok: false, error: calib.error };
    }
    var delay = numberValue("delay", 0, 0, 30, false);
    if (delay.error) {
      return { ok: false, error: delay.error };
    }
    return { ok: true, scene: scene, seconds: seconds.value, calib: calib.value, delay: delay.value };
  }

  function phaseAt(t, params) {
    if (t < params.delay) {
      return { phase: "delay" };
    }
    if (params.scene === "calib" || t < params.delay + params.calib) {
      var k = Math.floor((t - params.delay) / 0.5);
      return { phase: "calib", white: k % 2 === 0 };
    }
    if (t < params.delay + params.calib + params.seconds) {
      return { phase: "scene", sceneT: t - params.delay - params.calib };
    }
    return { phase: "end" };
  }

  function drawStatic(ctx, w, h) {
    ctx.fillStyle = "rgb(40,40,40)";
    ctx.fillRect(0, 0, w, h);
    var rects = staticRects(w, h);
    for (var i = 0; i < rects.length; i += 1) {
      var rect = rects[i];
      ctx.fillStyle = "rgb(" + rect[4] + "," + rect[4] + "," + rect[4] + ")";
      ctx.fillRect(rect[0], rect[1], rect[2], rect[3]);
    }
  }

  function rowWords(ctx, startX, right, y, lineHeight, scale, row, title) {
    var next = xorshift32((0x01234567 ^ Math.imul(row + 1, 0x9E3779B1)) >>> 0);
    var wordHeight = Math.round((title ? 20 : 14) * scale);
    var gap = Math.round(10 * scale);
    var x = startX;
    ctx.fillStyle = title ? "rgb(20,20,20)" : "rgb(60,60,60)";
    while (x < right) {
      var wordWidth = Math.round((20 + next() % 120) * scale);
      if (wordWidth < 1) {
        wordWidth = 1;
      }
      var wordY = y + Math.round((lineHeight - wordHeight) / 2);
      ctx.fillRect(x, wordY, wordWidth, wordHeight);
      x += wordWidth + gap;
      if (gap === 0 && wordWidth === 0) {
        break;
      }
    }
  }

  function drawScroll(ctx, w, h, t) {
    var scale = h / 1080;
    var lineHeight = Math.round(28 * scale);
    if (lineHeight < 1) {
      lineHeight = 1;
    }
    var sidebar = Math.round(240 * scale);
    ctx.fillStyle = "rgb(250,250,250)";
    ctx.fillRect(0, 0, w, h);
    ctx.fillStyle = "rgb(225,225,225)";
    ctx.fillRect(0, 0, sidebar, h);

    var offset = (1200 * scale * t) % (lineHeight * 1000);
    var first = Math.max(0, Math.floor((offset - lineHeight) / lineHeight));
    var last = Math.ceil((h + offset) / lineHeight);
    var startX = Math.round(280 * scale);
    var right = w - Math.round(60 * scale);
    for (var i = first; i <= last; i += 1) {
      var y = i * lineHeight - offset;
      if (y < -lineHeight || y >= h) {
        continue;
      }
      rowWords(ctx, startX, right, y, lineHeight, scale, i, i % 12 === 0);
    }
  }

  function defaultCanvasFactory(width, height) {
    if (typeof OffscreenCanvas === "function") {
      return new OffscreenCanvas(width, height);
    }
    if (typeof document !== "undefined" && document.createElement) {
      var canvas = document.createElement("canvas");
      canvas.width = width;
      canvas.height = height;
      return canvas;
    }
    throw new Error("drawVideo requires a canvas factory outside a browser");
  }

  function drawVideo(ctx, w, h, t, canvasFactory) {
    var factory = canvasFactory || defaultCanvasFactory;
    var offscreen = factory(240, 135);
    offscreen.width = 240;
    offscreen.height = 135;
    var offCtx = offscreen.getContext("2d");
    var image = offCtx.createImageData(240, 135);
    for (var y = 0; y < 135; y += 1) {
      for (var x = 0; x < 240; x += 1) {
        var value = Math.round(128 + 60 * Math.sin(x / 9 + t * 2.1) + 60 * Math.sin(y / 7 - t * 1.7));
        value = Math.max(0, Math.min(255, value));
        var index = (y * 240 + x) * 4;
        image.data[index] = value;
        image.data[index + 1] = 255 - value;
        image.data[index + 2] = (value * 2) & 255;
        image.data[index + 3] = 255;
      }
    }
    offCtx.putImageData(image, 0, 0);
    ctx.imageSmoothingEnabled = true;
    ctx.drawImage(offscreen, 0, 0, w, h);

    var radius = 0.06 * h;
    function bounce(min, max, speed) {
      var span = max - min;
      if (span <= 0) {
        return min;
      }
      var distance = (speed * t) % (2 * span);
      return min + (distance <= span ? distance : 2 * span - distance);
    }
    var cx = bounce(radius, w - radius, 700 * (h / 1080));
    var cy = bounce(radius, h - radius, 500 * (h / 1080));
    ctx.fillStyle = "#fff";
    ctx.beginPath();
    ctx.arc(cx, cy, radius, 0, Math.PI * 2);
    ctx.fill();
  }

  function drawDrag(ctx, w, h, t, bgCanvas) {
    ctx.drawImage(bgCanvas, 0, 0);
    var scale = h / 1080;
    var ww = Math.round(0.33 * w);
    var wh = Math.round(0.39 * h);
    var x = Math.round((w - ww) / 2 * (1 + Math.sin(2 * Math.PI * t / 4)));
    var y = Math.round((h - wh) / 2 * (1 + Math.sin(2 * Math.PI * t / 3)));
    var titleHeight = Math.round(0.03 * h);
    ctx.fillStyle = "rgb(90,90,90)";
    ctx.fillRect(x, y, ww, wh);
    ctx.fillStyle = "rgb(200,200,200)";
    ctx.fillRect(x + 1, y + 1, Math.max(0, ww - 2), Math.max(0, titleHeight - 1));
    ctx.fillStyle = "rgb(245,245,245)";
    ctx.fillRect(x + 1, y + titleHeight, Math.max(0, ww - 2), Math.max(0, wh - titleHeight - 1));

    var lineHeight = Math.round(28 * scale);
    if (lineHeight < 1) {
      lineHeight = 1;
    }
    var startX = x + Math.round(16 * scale);
    var right = x + ww - Math.round(16 * scale);
    for (var i = 0; i < 8; i += 1) {
      rowWords(ctx, startX, right, y + titleHeight + i * lineHeight, lineHeight, scale, i, i % 12 === 0);
    }
  }

  return {
    xorshift32: xorshift32,
    staticRects: staticRects,
    parseParams: parseParams,
    phaseAt: phaseAt,
    drawStatic: drawStatic,
    drawScroll: drawScroll,
    drawVideo: drawVideo,
    drawDrag: drawDrag
  };
});
