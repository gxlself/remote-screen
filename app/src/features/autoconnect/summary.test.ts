import { percentile, summarize } from './summary';

it('returns null for an empty percentile', () => {
  expect(percentile([], 50)).toBeNull();
});

it('uses the nearest-rank percentile', () => {
  const values = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
  expect(percentile(values, 50)).toBe(5);
  expect(percentile(values, 95)).toBe(10);
});

it('summarizes every field', () => {
  expect(
    summarize([
      { rttMs: 10, fps: 60, decodeMs: 2 },
      { rttMs: 20, fps: 50, decodeMs: 4 },
      { rttMs: 30, fps: 40, decodeMs: 6 },
    ]),
  ).toEqual({
    samples: 3,
    rttP50: 20,
    rttP95: 30,
    fpsAvg: 50,
    decodeP50: 4,
    decodeP95: 6,
  });
});
