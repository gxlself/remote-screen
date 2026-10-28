import { sessions } from '../../i18n/zh/sessions';
import { FIXTURE_NOW } from '../../native/rscore/fixtures';
import { formatDurationWords } from '../format/duration';
import type { HistoryEntry } from './history';
import { historyLine } from './historyText';

const MINUTE = 60 * 1000 * 1000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

function entry(overrides: Partial<HistoryEntry> = {}): HistoryEntry {
  return {
    id: 'remote:1',
    kind: 'remote',
    targetId: 'h7v2m9q3k8x4tb1a',
    name: '工位 Windows',
    startedAtUs: FIXTURE_NOW - 2 * HOUR,
    endedAtUs: FIXTURE_NOW - HOUR,
    durationS: 4320,
    ...overrides,
  };
}

beforeEach(() => {
  jest.spyOn(Date.prototype, 'getTimezoneOffset').mockReturnValue(-480);
});

afterEach(() => {
  jest.restoreAllMocks();
});

it('describes yesterday remote control through the Shanghai relay', () => {
  const line = historyLine(
    entry({
      endedAtUs: FIXTURE_NOW - DAY,
      path: 'tailnetDerp',
      derpRegion: '上海',
    }),
    FIXTURE_NOW,
  );

  expect(line.subtitle).toBe('远控 · 1 小时 12 分 · 经上海中继');
  expect(line.when).toBe('昨天');
  expect(line.a11y).toBe('重新连接工位 Windows，远控，1 小时 12 分，昨天');
});

it('uses HH:mm for a terminal session earlier today', () => {
  const line = historyLine(
    entry({
      kind: 'ssh',
      targetId: 'gpu-train-02',
      name: 'gpu-train-02',
      endedAtUs: FIXTURE_NOW - 46 * MINUTE,
      durationS: 18 * 60,
    }),
    FIXTURE_NOW,
  );

  expect(line.subtitle).toBe('终端 · 18 分钟');
  expect(line.when).toBe('10:12');
  expect(line.icon).toBe('terminal');
});

it('adds the unclean suffix to the subtitle and spoken label', () => {
  const line = historyLine(entry({ unclean: true }), FIXTURE_NOW);

  expect(line.subtitle).toBe('远控 · 1 小时 12 分 · 上次没正常结束');
  expect(line.a11y).toContain('上次没正常结束');
});

it('uses month and day for an older session', () => {
  expect(
    historyLine(entry({ endedAtUs: FIXTURE_NOW - 3 * DAY }), FIXTURE_NOW).when,
  ).toBe('10月17日');
});

it('falls back to a plain relay label when the region is missing', () => {
  expect(
    historyLine(
      entry({ path: 'tailnetDerp', derpRegion: undefined }),
      FIXTURE_NOW,
    ).subtitle,
  ).toBe('远控 · 1 小时 12 分 · 中继');
});

it('describes a direct intranet session', () => {
  expect(
    historyLine(entry({ path: 'intranet', derpRegion: undefined }), FIXTURE_NOW)
      .subtitle,
  ).toBe('远控 · 1 小时 12 分 · 经内网');
});

it('names adapter sessions by their protocol', () => {
  const vnc = historyLine(
    entry({
      id: 'adapter:1',
      kind: 'adapter',
      protocol: 'vnc',
      durationS: 600,
    }),
    FIXTURE_NOW,
  );
  expect(vnc.subtitle).toBe(
    sessions.history.subtitle.adapter(
      sessions.history.adapterKind.vnc,
      formatDurationWords(600),
    ),
  );
  expect(vnc.icon).toBe('computer');
  expect(vnc.a11y).toContain(sessions.history.adapterKind.vnc);

  const rdp = historyLine(
    entry({ kind: 'adapter', protocol: 'rdp', unclean: true }),
    FIXTURE_NOW,
  );
  expect(rdp.subtitle).toContain(sessions.history.adapterKind.rdp);
  expect(rdp.subtitle).toContain(sessions.history.uncleanSuffix);
});
