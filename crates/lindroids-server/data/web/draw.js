// Drawing a window's frames: the changed tiles the server sends (LZ4, RGBA) go into the window's
// canvas. On the GPU where the browser has WebGL: each tile is uploaded into a texture
// (texSubImage2D) and one triangle draws it, so the browser composites the canvas without a copy
// through the CPU. Without WebGL, putImageData.
//
// The page runs this in a worker that owns the canvas (OffscreenCanvas): decoding and drawing a
// frame then never hold up a touch, a key or a mouse move on the page's main thread. Browsers
// without OffscreenCanvas run it in the page itself (index.html loads it there too).
'use strict';

/// A drawer: `post` gets its answers, the returned function takes the page's messages.
/// - {t:'canvas', win, canvas}: a window's canvas (an OffscreenCanvas in a worker).
/// - {t:'tiles', win, width, height, rects, data, off}: a frame; `data` (an ArrayBuffer) holds,
///   from `off`, the raw size (u32 LE) and the LZ4 block. Answered by {t:'drawn', win, ms}.
/// - {t:'close', win}.
/// It also posts {t:'mode', win, mode, renderer} once per canvas, and {t:'lost', win} when the
/// GPU context is lost and back (the page then asks the server for a whole frame).
function lindroidsDrawer(post) {
  const wins = new Map();
  let raw = new Uint8Array(0);

  function lz4(src, dst) {
    let s = 0, d = 0;
    const end = src.length;
    while (s < end) {
      const token = src[s++];
      let lit = token >> 4;
      if (lit === 15) { let b; do { b = src[s++]; lit += b; } while (b === 255); }
      if (lit) {
        if (lit < 16) { for (let k = 0; k < lit; k++) dst[d + k] = src[s + k]; }
        else dst.set(src.subarray(s, s + lit), d);
        s += lit; d += lit;
      }
      if (s >= end) break;
      const offset = src[s] | (src[s + 1] << 8);
      s += 2;
      let len = token & 15;
      if (len === 15) { let b; do { b = src[s++]; len += b; } while (b === 255); }
      len += 4;
      let m = d - offset;
      if (offset >= len) {
        if (len < 32) { for (let k = 0; k < len; k++) dst[d + k] = dst[m + k]; }
        else dst.copyWithin(d, m, m + len);
        d += len;
      } else if (len < 32) {
        for (let k = 0; k < len; k++) dst[d++] = dst[m++];
      } else {
        // A run that repeats its last `offset` bytes (a flat background is one pixel, repeated):
        // copy the pattern in chunks that double, not a byte at a time.
        while (len > 0) {
          const n = Math.min(d - m, len);
          dst.copyWithin(d, m, m + n);
          d += n; len -= n;
        }
      }
    }
    return d;
  }

  const VS = 'attribute vec2 p; void main() { gl_Position = vec4(p, 0.0, 1.0); }';
  // The canvas and the texture have the same size: each pixel takes its own texel, unfiltered.
  const FS = `#ifdef GL_FRAGMENT_PRECISION_HIGH
precision highp float;
#else
precision mediump float;
#endif
uniform sampler2D t; uniform vec2 s;
void main() { gl_FragColor = texture2D(t, vec2(gl_FragCoord.x, s.y - gl_FragCoord.y) / s); }`;
  const GL_OPTS = { alpha: false, antialias: false, depth: false, stencil: false, premultipliedAlpha: true,
    preserveDrawingBuffer: false, desynchronized: true, powerPreference: 'high-performance' };

  function glSetup(w) {
    const gl = w.gl;
    const shader = (type, src) => {
      const sh = gl.createShader(type);
      gl.shaderSource(sh, src);
      gl.compileShader(sh);
      if (!gl.getShaderParameter(sh, gl.COMPILE_STATUS) && !gl.isContextLost()) throw new Error(gl.getShaderInfoLog(sh));
      return sh;
    };
    const prog = gl.createProgram();
    gl.attachShader(prog, shader(gl.VERTEX_SHADER, VS));
    gl.attachShader(prog, shader(gl.FRAGMENT_SHADER, FS));
    gl.bindAttribLocation(prog, 0, 'p');
    gl.linkProgram(prog);
    if (!gl.getProgramParameter(prog, gl.LINK_STATUS) && !gl.isContextLost()) throw new Error(gl.getProgramInfoLog(prog));
    gl.useProgram(prog);
    // One triangle larger than the screen covers it.
    gl.bindBuffer(gl.ARRAY_BUFFER, gl.createBuffer());
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, gl.createTexture());
    for (const [k, v] of [[gl.TEXTURE_MIN_FILTER, gl.NEAREST], [gl.TEXTURE_MAG_FILTER, gl.NEAREST],
      [gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE], [gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE]]) gl.texParameteri(gl.TEXTURE_2D, k, v);
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 4);
    gl.uniform1i(gl.getUniformLocation(prog, 't'), 0);
    w.size = gl.getUniformLocation(prog, 's');
    gl.disable(gl.BLEND);
    gl.disable(gl.DITHER);
    w.texSize = [0, 0];
  }

  // Which context to draw with, decided once on a canvas of its own (a canvas keeps the first
  // kind of context it gives): WebGL 2, else WebGL, else 2D. A software GL (SwiftShader,
  // llvmpipe: a machine without a usable GPU) is slower than putImageData, so 2D then.
  let choice = null, wanted = 'auto';
  function choose(make) {
    if (choice) return choice;
    choice = { kind: '2d', renderer: '' };
    if (wanted === '2d') return choice;
    for (const kind of ['webgl2', 'webgl']) {
      let gl = null;
      try { gl = make().getContext(kind, GL_OPTS); } catch (e) { /* none */ }
      if (!gl) continue;
      let renderer = '';
      try {
        const ext = gl.getExtension('WEBGL_debug_renderer_info');
        renderer = String(gl.getParameter(ext ? ext.UNMASKED_RENDERER_WEBGL : gl.RENDERER));
      } catch (e) { /* not said */ }
      const lose = gl.getExtension('WEBGL_lose_context');
      if (lose) lose.loseContext();
      if (wanted !== 'gl' && /swiftshader|llvmpipe|softpipe|software/i.test(renderer)) {
        choice = { kind: '2d', renderer: `${renderer}: software, not used` };
        return choice;
      }
      choice = { kind, renderer };
      return choice;
    }
    return choice;
  }

  function attach(win, canvas) {
    const w = { win, canvas, gl: null, ctx: null, lost: false, texSize: [0, 0], size: null };
    const make = () => (typeof OffscreenCanvas === 'function' ? new OffscreenCanvas(1, 1) : document.createElement('canvas'));
    const c = choose(make);
    if (c.kind !== '2d') {
      try {
        w.gl = canvas.getContext(c.kind, GL_OPTS);
        if (w.gl) glSetup(w);
      } catch (e) {
        w.gl = null;
      }
    }
    if (w.gl) {
      canvas.addEventListener('webglcontextlost', (e) => { e.preventDefault(); w.lost = true; });
      canvas.addEventListener('webglcontextrestored', () => {
        w.lost = false;
        try { glSetup(w); } catch (err) { return; }
        post({ t: 'lost', win });
      });
      post({ t: 'mode', win, mode: c.kind, renderer: c.renderer });
    } else {
      w.ctx = canvas.getContext('2d', { alpha: false, desynchronized: true });
      post({ t: 'mode', win, mode: w.ctx ? '2d' : 'none', renderer: c.renderer });
    }
    wins.set(win, w);
  }

  function draw(m) {
    const w = wins.get(m.win);
    const t0 = performance.now();
    if (!w) return post({ t: 'drawn', win: m.win, ms: 0 });
    const src = new Uint8Array(m.data, m.off + 4);
    const head = new DataView(m.data, m.off, 4);
    const size = head.getUint32(0, true);
    if (raw.length < size) raw = new Uint8Array(Math.max(size, raw.length * 2));
    lz4(src, raw);
    const t1 = performance.now();
    const { width, height } = m;
    if (w.gl) {
      const gl = w.gl;
      if (!w.lost) {
        if (w.canvas.width !== width || w.canvas.height !== height) { w.canvas.width = width; w.canvas.height = height; }
        if (w.texSize[0] !== width || w.texSize[1] !== height) {
          gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, width, height, 0, gl.RGBA, gl.UNSIGNED_BYTE, null);
          w.texSize = [width, height];
          gl.viewport(0, 0, width, height);
          gl.uniform2f(w.size, width, height);
        }
        let off = 0;
        for (const [x, y, rw, rh] of m.rects) {
          const n = rw * rh * 4;
          gl.texSubImage2D(gl.TEXTURE_2D, 0, x, y, rw, rh, gl.RGBA, gl.UNSIGNED_BYTE, raw.subarray(off, off + n));
          off += n;
        }
        gl.drawArrays(gl.TRIANGLES, 0, 3);
      }
    } else if (w.ctx) {
      if (w.canvas.width !== width || w.canvas.height !== height) { w.canvas.width = width; w.canvas.height = height; }
      const px = new Uint8ClampedArray(raw.buffer);
      let off = 0;
      for (const [x, y, rw, rh] of m.rects) {
        const n = rw * rh * 4;
        w.ctx.putImageData(new ImageData(px.subarray(off, off + n), rw, rh), x, y);
        off += n;
      }
    }
    post({ t: 'drawn', win: m.win, ms: performance.now() - t0, decode: t1 - t0, bytes: m.data.byteLength - m.off, pixels: size / 4 });
  }

  return (m) => {
    switch (m.t) {
      case 'canvas': return attach(m.win, m.canvas);
      case 'tiles': return draw(m);
      case 'close': return wins.delete(m.win);
      case 'config': wanted = m.draw || 'auto'; choice = null; return;
      default: return;
    }
  };
}

if (typeof WorkerGlobalScope !== 'undefined' && self instanceof WorkerGlobalScope) {
  const take = lindroidsDrawer((m) => self.postMessage(m));
  self.onmessage = (e) => take(e.data);
}
