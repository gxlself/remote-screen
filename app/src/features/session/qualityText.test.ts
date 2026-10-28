import type { CoreEventOf, DialAttempt } from '../../native/rscore/types';
import {
  QUALITY_OPTIONS,
  pathLine,
  qualityDesc,
  statRows,
  statsLabel,
} from './qualityText';

const stats: CoreEventOf<'session.stats'> = {
  type: 'session.stats',
  sessionId: 's1',
  rttMs: 6,
  fps: 60,
  bitrateKbps: 18_400,
  decodeMs: 3,
  lossPct: 0.1,
  width: 2560,
  height: 1600,
  codec: 'hevc',
  path: 'tailnetDirect',
  transport: 'quic',
};

const direct: DialAttempt = {
  path: 'tailnetDirect',
  addr: '192.168.31.20:41641',
  state: 'ok',
};

describe('qualityText', () => {
  it('keeps the canvas order and provides a description for every option', () => {
    expect(QUALITY_OPTIONS).toEqual(['auto', 'smooth', 'clear', 'eco']);
    for (const quality of QUALITY_OPTIONS) {
      expect(qualityDesc(quality).length).toBeGreaterThan(0);
    }
    expect(qualityDesc('auto')).toBe(
      '按网络自动调整。网络好时又清楚又顺滑，变差时先降清晰度，再降流畅度。',
    );
  });

  it('formats all six statistics rows', () => {
    expect(statRows(stats).map(row => row.value)).toEqual([
      '6 ms',
      '60 fps',
      '18.4 Mbps',
      '2560×1600',
      '0.1%',
      'HEVC',
    ]);
  });

  it('uses em dashes for every value without stats', () => {
    const rows = statRows(null);
    expect(rows.map(row => row.value)).toEqual(['—', '—', '—', '—', '—', '—']);
    expect(statsLabel(rows)).toBe(
      '延迟 —，帧率 —，码率 —，分辨率 —，丢包 —，编码 —',
    );
  });

  it('builds the spoken statistics sentence', () => {
    expect(statsLabel(statRows(stats))).toBe(
      '延迟 6 毫秒，帧率 60，码率 18.4 Mbps，分辨率 2560×1600，丢包 0.1%，编码 HEVC',
    );
  });

  it('builds the path line with and without a successful address', () => {
    expect(pathLine(stats, direct)).toBe('直连 192.168.31.20:41641');
    expect(pathLine(stats)).toBe('直连');
    expect(pathLine(stats, { ...direct, state: 'failed' })).toBe('直连');
    expect(pathLine(stats, { ...direct, addr: '' })).toBe('直连');
  });

  it('labels H.264 and falls back to the successful dial path', () => {
    expect(statRows({ ...stats, codec: 'h264' })[5].value).toBe('H.264');
    expect(pathLine(null, direct)).toBe('直连 192.168.31.20:41641');
  });
});
