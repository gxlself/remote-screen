import { recordDenied, remainingRequests, resetDenied } from './deniedQuota';

const HOST = 'h1';
const OTHER = 'h2';
const NOW = 1_800_000_000_000;

beforeEach(() => {
  resetDenied();
});

it('drops the remaining count after each denial', () => {
  expect(remainingRequests(HOST, NOW)).toBe(3);
  recordDenied(HOST, NOW);
  expect(remainingRequests(HOST, NOW)).toBe(2);
  recordDenied(HOST, NOW + 1000);
  expect(remainingRequests(HOST, NOW + 1000)).toBe(1);
  recordDenied(HOST, NOW + 2000);
  expect(remainingRequests(HOST, NOW + 2000)).toBe(0);
});

it('restores a request after ten minutes and one second', () => {
  recordDenied(HOST, NOW);
  expect(remainingRequests(HOST, NOW)).toBe(2);
  expect(remainingRequests(HOST, NOW + 10 * 60 * 1000 + 1000)).toBe(3);
});

it('keeps hosts independent', () => {
  recordDenied(HOST, NOW);
  recordDenied(HOST, NOW);
  expect(remainingRequests(OTHER, NOW)).toBe(3);
  expect(remainingRequests(HOST, NOW)).toBe(1);
});
