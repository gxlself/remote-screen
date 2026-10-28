import { describe, expect, it } from 'vitest';

import { zh } from '../i18n';
import { WINDOW_LABELS } from '../platform/types';
import { ROUTES } from './routes';
import { SETTINGS_SECTIONS } from '../pages/settings/sections';
import type { PageId } from './router';

const PAGE_IDS: PageId[] = [
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

describe('ROUTES', () => {
  it('covers every page', () => {
    expect(Object.keys(ROUTES).sort()).toEqual([...PAGE_IDS].sort());
  });

  it('contains every window label', () => {
    for (const label of WINDOW_LABELS) {
      expect(ROUTES[label]).toBeDefined();
    }
  });
});

describe('SETTINGS_SECTIONS', () => {
  it('is ordered as specified', () => {
    expect(SETTINGS_SECTIONS.map((section) => section.id)).toEqual([
      'general',
      'network',
      'devices',
      'pairing',
      'permissions',
      'history',
      'about',
    ]);
  });

  it('has a label for every section', () => {
    for (const section of SETTINGS_SECTIONS) {
      expect(zh.common.settingsNav[section.id]).toBeTruthy();
    }
  });
});
