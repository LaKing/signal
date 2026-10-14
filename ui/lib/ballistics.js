// Meter ballistics: the M2.5-1 ones (crates/signal-app/src/meter.rs), line
// for line. From per-block peak and sum of squares: the peak in dBFS with a
// 1.5 s hold, the RMS over 300 ms, and a clip indicator latched until
// cleared. Time is counted in frames, so the ballistics are the same however
// the blocks arrive.

/** How long the held peak stays, in seconds. */
export const PEAK_HOLD_SECONDS = 1.5;

/** The RMS window, in seconds. */
export const RMS_WINDOW_SECONDS = 0.3;

/** `20 log10(x)`; −∞ for silence. */
export function dbfs(x) {
  return x > 0 ? 20 * Math.log10(x) : -Infinity;
}

/** One channel's ballistics (`ChannelMeter` in meter.rs). */
export class ChannelMeter {
  /** A meter for blocks at `sampleRate`. */
  constructor(sampleRate) {
    const frames = (seconds) => Math.round(seconds * sampleRate);
    this.holdFrames = frames(PEAK_HOLD_SECONDS);
    this.windowFrames = Math.max(frames(RMS_WINDOW_SECONDS), 1);
    // Peaks are f32 in Rust (and on the wire): keep them as f32 values.
    this.peak = 0;
    this.held = 0;
    /** The end frame of the block that set `held`. */
    this.heldAt = 0;
    this.blocks = [];
    this.frames = 0;
    this.sumSquares = 0;
    this.clip = false;
  }

  /**
   * Takes one block `{firstFrame, frames, peak, sumSquares}`. A peak of 1.0
   * or more latches the clip indicator (a clamped output sample reads
   * exactly 1.0).
   */
  push({ firstFrame, frames, peak, sumSquares }) {
    const end = firstFrame + frames;
    const p = Math.fround(peak);
    this.peak = p;
    if (p >= this.held || Math.max(0, end - this.heldAt) > this.holdFrames) {
      this.held = p;
      this.heldAt = end;
    }
    if (p >= 1.0) {
      this.clip = true;
    }
    this.blocks.push([frames, sumSquares]);
    this.frames += frames;
    this.sumSquares += sumSquares;
    // Keep the newest blocks that cover the window.
    while (this.blocks.length > 0) {
      const [oldest, sum] = this.blocks[0];
      if (this.frames - oldest < this.windowFrames) break;
      this.blocks.shift();
      this.frames -= oldest;
      this.sumSquares -= sum;
    }
  }

  /** Clears the clip indicator. */
  clearClip() {
    this.clip = false;
  }

  /** What it shows now, in dBFS: `{peak, held, rms, clip}`. */
  reading() {
    const rms = this.frames > 0 ? Math.sqrt(Math.max(this.sumSquares, 0) / this.frames) : 0;
    return { peak: dbfs(this.peak), held: dbfs(this.held), rms: dbfs(rms), clip: this.clip };
  }
}

/**
 * A meter frame of the UI protocol (kind 1, decoded) as one block per
 * channel: the frame carries the RMS over `extra` frames, so the block's sum
 * of squares is `rms² · frames`.
 */
export function blocksFromFrame(frame) {
  return frame.values.map(({ peak, rms }) => ({
    firstFrame: frame.firstFrame,
    frames: frame.extra,
    peak,
    sumSquares: rms * rms * frame.extra,
  }));
}
