import { common } from '../../i18n/zh/common';

const { time } = common;
const US_PER_MS = 1000;
const MS_PER_MINUTE = 60 * 1000;
const MS_PER_HOUR = 60 * MS_PER_MINUTE;
const MS_PER_DAY = 24 * MS_PER_HOUR;

function localOffset(tzOffsetMin?: number): number {
  return tzOffsetMin ?? -new Date().getTimezoneOffset();
}

function localMilliseconds(us: number, tzOffsetMin?: number): number {
  return Math.floor(us / US_PER_MS) + localOffset(tzOffsetMin) * MS_PER_MINUTE;
}

function localDate(us: number, tzOffsetMin?: number): Date {
  return new Date(localMilliseconds(us, tzOffsetMin));
}

function localDayIndex(us: number, tzOffsetMin?: number): number {
  return Math.floor(localMilliseconds(us, tzOffsetMin) / MS_PER_DAY);
}

function monthDay(date: Date): string {
  return time.monthDay(date.getUTCMonth() + 1, date.getUTCDate());
}

export function formatRelativePast(
  us: number,
  nowUs: number,
  tzOffsetMin?: number,
): string {
  const elapsedMs = (nowUs - us) / US_PER_MS;
  if (elapsedMs < MS_PER_MINUTE) {
    return time.justNow;
  }
  if (elapsedMs < MS_PER_HOUR) {
    return time.minutesAgo(Math.floor(elapsedMs / MS_PER_MINUTE));
  }
  if (elapsedMs < MS_PER_DAY) {
    return time.hoursAgo(Math.floor(elapsedMs / MS_PER_HOUR));
  }
  if (
    localDayIndex(nowUs, tzOffsetMin) - localDayIndex(us, tzOffsetMin) ===
    1
  ) {
    return time.yesterday;
  }
  return monthDay(localDate(us, tzOffsetMin));
}

export function formatHm(us: number, tzOffsetMin?: number): string {
  const date = localDate(us, tzOffsetMin);
  return `${String(date.getUTCHours()).padStart(2, '0')}:${String(
    date.getUTCMinutes(),
  ).padStart(2, '0')}`;
}

export function formatDayHeader(
  us: number,
  nowUs: number,
  tzOffsetMin?: number,
): string {
  const date = localDate(us, tzOffsetMin);
  const dayOffset =
    localDayIndex(nowUs, tzOffsetMin) - localDayIndex(us, tzOffsetMin);
  const weekday = time.weekdays[date.getUTCDay()];
  const calendarDate = `${monthDay(date)} ${weekday}`;

  if (dayOffset === 0) {
    return `${time.today} · ${calendarDate}`;
  }
  if (dayOffset === 1) {
    return `${time.yesterday} · ${calendarDate}`;
  }
  return calendarDate;
}
