import type { IconName } from '../../components/icons/Icon';
import { common } from '../../i18n/zh/common';
import { sessions } from '../../i18n/zh/sessions';
import { formatDurationWords } from '../format/duration';
import { formatHm } from '../format/time';
import type { HistoryEntry } from './history';

export type HistoryLine = {
  title: string;
  subtitle: string;
  when: string;
  icon: IconName;
  a11y: string;
};

const US_PER_MS = 1000;
const MS_PER_DAY = 24 * 60 * 60 * 1000;

function localDayIndex(us: number): number {
  const localMs =
    Math.floor(us / US_PER_MS) - new Date().getTimezoneOffset() * 60 * 1000;
  return Math.floor(localMs / MS_PER_DAY);
}

function whenText(endedAtUs: number, nowUs: number): string {
  const dayOffset = localDayIndex(nowUs) - localDayIndex(endedAtUs);
  if (dayOffset === 0) {
    return formatHm(endedAtUs);
  }
  if (dayOffset === 1) {
    return common.time.yesterday;
  }
  const date = new Date(
    Math.floor(endedAtUs / US_PER_MS) -
      new Date().getTimezoneOffset() * 60 * 1000,
  );
  return common.time.monthDay(date.getUTCMonth() + 1, date.getUTCDate());
}

function pathText(entry: HistoryEntry): string | null {
  if (entry.kind !== 'remote') {
    return null;
  }
  switch (entry.path) {
    case 'tailnetDirect':
      return sessions.history.path.direct;
    case 'tailnetDerp': {
      const region = entry.derpRegion?.trim();
      return region === undefined || region === ''
        ? sessions.history.path.relayUnknown
        : sessions.history.path.relay(region);
    }
    case 'intranet':
      return sessions.history.path.intranet;
    case 'lan':
      return sessions.history.path.lan;
    case 'manual':
      return sessions.history.path.manual;
    default:
      return null;
  }
}

function kindText(entry: HistoryEntry): string {
  return entry.kind === 'adapter' && entry.protocol !== undefined
    ? sessions.history.adapterKind[entry.protocol]
    : sessions.history.kind[entry.kind];
}

function subtitleText(
  entry: HistoryEntry,
  kind: string,
  duration: string,
  path: string | null,
): string {
  switch (entry.kind) {
    case 'remote':
      return sessions.history.subtitle.remote(duration, path);
    case 'adapter':
      return sessions.history.subtitle.adapter(kind, duration);
    default:
      return sessions.history.subtitle.ssh(duration);
  }
}

export function historyLine(entry: HistoryEntry, nowUs: number): HistoryLine {
  const duration = formatDurationWords(entry.durationS);
  const when = whenText(entry.endedAtUs, nowUs);
  const kind = kindText(entry);
  const unclean = entry.unclean === true;
  const uncleanSuffix = unclean ? sessions.history.uncleanSuffix : '';
  const path = pathText(entry);
  const subtitle = subtitleText(entry, kind, duration, path) + uncleanSuffix;

  return {
    title: entry.name,
    subtitle,
    when,
    icon: entry.kind === 'ssh' ? 'terminal' : 'computer',
    a11y: sessions.history.reconnectA11y(
      entry.name,
      kind,
      duration,
      when,
      unclean,
    ),
  };
}
