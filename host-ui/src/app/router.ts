import { useSyncExternalStore } from 'react';

import type { WindowLabel } from '../platform/types';

export type { WindowLabel } from '../platform/types';

export type PageId = WindowLabel | 'dev';

export interface Route {
  page: PageId;
  section?: string;
  query: Record<string, string>;
}

export interface PageProps {
  route: Route;
}

const PAGE_IDS: readonly PageId[] = [
  'bg',
  'menu',
  'capsule',
  'border',
  'confirm',
  'pair',
  'settings',
  'setup',
  'update',
  'dev',
];

function isPageId(value: string): value is PageId {
  return (PAGE_IDS as readonly string[]).includes(value);
}

export function parseHash(hash: string): Route {
  if (hash === '' || hash === '#' || hash === '#/') {
    return { page: 'dev', query: {} };
  }

  const raw = hash.startsWith('#/') ? hash.slice(2) : hash.startsWith('#') ? hash.slice(1) : hash;
  const queryIndex = raw.indexOf('?');
  const rawPath = queryIndex === -1 ? raw : raw.slice(0, queryIndex);
  const rawQuery = queryIndex === -1 ? '' : raw.slice(queryIndex + 1);
  const segments = rawPath.split('/').filter((segment) => segment.length > 0);
  const first = segments[0];
  if (first === undefined || !isPageId(first)) {
    return { page: 'dev', query: {} };
  }

  const query: Record<string, string> = {};
  for (const pair of rawQuery.split('&')) {
    if (pair.length === 0) {
      continue;
    }
    const equals = pair.indexOf('=');
    const rawKey = equals === -1 ? pair : pair.slice(0, equals);
    const rawValue = equals === -1 ? '' : pair.slice(equals + 1);
    try {
      query[decodeURIComponent(rawKey)] = decodeURIComponent(rawValue);
    } catch {
      query[rawKey] = rawValue;
    }
  }

  const section = first === 'settings' ? segments[1] : undefined;
  const route: Route = { page: first, query };
  if (section !== undefined) {
    route.section = section;
  }
  return route;
}

export function buildHash(route: Route): string {
  let path = route.page;
  if (route.section !== undefined) {
    path += `/${route.section}`;
  }
  const query = Object.entries(route.query)
    .map(([key, value]) => `${encodeURIComponent(key)}=${encodeURIComponent(value)}`)
    .join('&');
  return query.length > 0 ? `#/${path}?${query}` : `#/${path}`;
}

export function navigate(path: string): void {
  location.hash = `#/${path.replace(/^#?\/?/, '')}`;
}

const subscribeToHash = (onStoreChange: () => void): (() => void) => {
  window.addEventListener('hashchange', onStoreChange);
  return () => window.removeEventListener('hashchange', onStoreChange);
};

const readHash = (): string => location.hash;

export function useRoute(): Route {
  const hash = useSyncExternalStore(subscribeToHash, readHash, () => '');
  return parseHash(hash);
}
