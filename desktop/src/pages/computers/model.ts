import type { PairedHost } from '@app/native/rscore/types';

import type { OpenSessionArgs } from '../../core/desktopApi';

/** A paired host. */
export type ComputerRow = {
  hostId: string;
  name: string;
  os: PairedHost['os'];
};

/** By name. */
export function computerRows(paired: PairedHost[]): ComputerRow[] {
  return paired
    .map((host) => ({ hostId: host.hostId, name: host.name, os: host.os }))
    .sort((a, b) => a.name.localeCompare(b.name));
}

/** DeviceGlyph draws macos and windows as a computer; anything else would be a phone. */
export function glyphPlatform(os: ComputerRow['os']): 'macos' | 'windows' {
  return os === 'windows' ? 'windows' : 'macos';
}

export function sessionOs(os: ComputerRow['os']): OpenSessionArgs['hostOs'] {
  return os === 'macos' || os === 'windows' ? os : 'unknown';
}
