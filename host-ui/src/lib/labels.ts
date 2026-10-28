import { zh } from '../i18n/zh';

const PATH_LABELS: Readonly<Record<string, string>> = zh.common.path;
const PERMISSION_LABELS: Readonly<Record<string, string>> = zh.common.permission;

function lookup(labels: Readonly<Record<string, string>>, key: string): string | undefined {
  return Object.hasOwn(labels, key) ? labels[key] : undefined;
}

export function pathLabel(path: string | null | undefined): string {
  if (path === null || path === undefined) {
    return '';
  }
  return lookup(PATH_LABELS, path) ?? path;
}

export function permissionLabel(permission: string): string {
  return lookup(PERMISSION_LABELS, permission) ?? permission;
}
