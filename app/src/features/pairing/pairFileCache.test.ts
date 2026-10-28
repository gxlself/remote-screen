import type { PairFileSummary } from '../../native/rscore/types';
import {
  clearPairFiles,
  recallPairFile,
  rememberPairFile,
} from './pairFileCache';

const T0 = 1_792_465_134_000;
const MINUTE_MS = 60 * 1000;

function summary(fileId: string): PairFileSummary {
  return {
    fileId,
    hostId: `host-${fileId}`,
    hostName: `Mac ${fileId}`,
    fp: 'w-LXse0ld21C5mCG6UpzNHUXoKVl-IiQwO9LWYDVFdQ',
    addrs: ['100.64.0.7:7843'],
    expiresAt: T0 * 1000 + 24 * 60 * MINUTE_MS * 1000,
    expired: false,
  };
}

afterEach(() => {
  clearPairFiles();
});

describe('pairFileCache', () => {
  it('round-trips a summary without sharing mutable arrays', () => {
    const value = summary('one');
    rememberPairFile(value, T0);
    value.addrs.push('mutated');

    expect(recallPairFile('one', T0)).toEqual(summary('one'));
  });

  it('stores only the summary fields, never a secret', () => {
    const leaked = {
      ...summary('one'),
      secret: 'fixture-only',
      text: 'rsp://pairfile?v=1&s=fixture-only',
    } as PairFileSummary;
    rememberPairFile(leaked, T0);

    expect(recallPairFile('one', T0)).toEqual(summary('one'));
  });

  it('expires after ten minutes and one second', () => {
    rememberPairFile(summary('one'), T0);
    expect(recallPairFile('one', T0 + 10 * MINUTE_MS - 1)).toBeDefined();
    expect(recallPairFile('one', T0 + 10 * MINUTE_MS + 1000)).toBeUndefined();
  });

  it('evicts the oldest entry when a ninth file is remembered', () => {
    for (let index = 0; index < 8; index += 1) {
      rememberPairFile(summary(`file-${index}`), T0 + index);
    }
    rememberPairFile(summary('file-8'), T0 + 8);

    expect(recallPairFile('file-0', T0 + 8)).toBeUndefined();
    expect(recallPairFile('file-1', T0 + 8)).toBeDefined();
    expect(recallPairFile('file-8', T0 + 8)).toBeDefined();
  });

  it('overwrites an existing file id and refreshes its recency', () => {
    rememberPairFile(summary('one'), T0);
    rememberPairFile(summary('two'), T0 + 1);
    const updated = { ...summary('one'), hostName: 'Updated Mac' };
    rememberPairFile(updated, T0 + 2);
    for (let index = 2; index < 8; index += 1) {
      rememberPairFile(summary(`file-${index}`), T0 + index);
    }
    rememberPairFile(summary('file-8'), T0 + 8);

    expect(recallPairFile('one', T0 + 8)?.hostName).toBe('Updated Mac');
    expect(recallPairFile('two', T0 + 8)).toBeUndefined();
  });
});
