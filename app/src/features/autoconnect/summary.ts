export type SessionSample = {
  rttMs: number;
  fps: number;
  decodeMs: number;
};

export type SessionSummary = {
  samples: number;
  rttP50: number | null;
  rttP95: number | null;
  fpsAvg: number | null;
  decodeP50: number | null;
  decodeP95: number | null;
};

/** Nearest-rank percentile. */
export function percentile(
  values: readonly number[],
  p: number,
): number | null {
  if (values.length === 0) {
    return null;
  }
  const sorted = [...values].sort((left, right) => left - right);
  const rank = Math.ceil((p / 100) * sorted.length);
  const index = Math.min(sorted.length - 1, Math.max(0, rank - 1));
  return sorted[index] ?? null;
}

function average(values: readonly number[]): number | null {
  if (values.length === 0) {
    return null;
  }
  return values.reduce((sum, value) => sum + value, 0) / values.length;
}

export function summarize(samples: readonly SessionSample[]): SessionSummary {
  const rtt = samples.map(sample => sample.rttMs);
  const fps = samples.map(sample => sample.fps);
  const decode = samples.map(sample => sample.decodeMs);
  return {
    samples: samples.length,
    rttP50: percentile(rtt, 50),
    rttP95: percentile(rtt, 95),
    fpsAvg: average(fps),
    decodeP50: percentile(decode, 50),
    decodeP95: percentile(decode, 95),
  };
}
