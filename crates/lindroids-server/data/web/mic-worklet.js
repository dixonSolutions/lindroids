// Lindroids: the microphone's samples, out of the audio thread. Loaded by lindroids.js
// (recordOpen) as an AudioWorklet module; it posts each channel's samples about every 20 ms
// as [Float32Array, ...] and outputs nothing (silence).
'use strict';

class LindroidsMic extends AudioWorkletProcessor {
  constructor() {
    super();
    this.size = Math.max(128, Math.round(sampleRate / 50));
    this.bufs = null;
    this.n = 0;
    this.port.onmessage = (e) => { if (e.data === 'stop') this.stopped = true; };
  }

  process(inputs) {
    if (this.stopped) return false;
    const input = inputs[0];
    if (!input || !input.length) return true;
    if (!this.bufs || this.bufs.length !== input.length) {
      this.bufs = input.map(() => new Float32Array(this.size));
      this.n = 0;
    }
    let off = 0;
    const len = input[0].length;
    while (off < len) {
      const take = Math.min(len - off, this.size - this.n);
      for (let c = 0; c < input.length; c++) this.bufs[c].set(input[c].subarray(off, off + take), this.n);
      this.n += take;
      off += take;
      if (this.n === this.size) {
        const out = this.bufs;
        this.port.postMessage(out, out.map((b) => b.buffer));
        this.bufs = input.map(() => new Float32Array(this.size));
        this.n = 0;
      }
    }
    return true;
  }
}

registerProcessor('lindroids-mic', LindroidsMic);
