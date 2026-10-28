import { common } from '../../i18n/zh/common';

const { time } = common;

export function formatClock(seconds: number): string {
  const total = Math.max(0, Math.floor(seconds));
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const secs = total % 60;
  const minuteText = String(minutes).padStart(2, '0');
  const secondText = String(secs).padStart(2, '0');

  if (hours > 0) {
    return `${hours}:${minuteText}:${secondText}`;
  }
  return `${minuteText}:${secondText}`;
}

export function formatDurationWords(seconds: number): string {
  const totalMinutes = Math.max(1, Math.floor(seconds / 60));
  const hours = Math.floor(totalMinutes / 60);
  const minutes = totalMinutes % 60;

  if (hours > 0) {
    return time.hourMinute(hours, minutes);
  }
  return time.minutes(totalMinutes);
}
