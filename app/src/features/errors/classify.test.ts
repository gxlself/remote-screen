import { classifyEnd } from './classify';
import type { BoardKey, GenericKey } from './classify';

type Case = {
  input: { code?: number; reason?: string };
  board: BoardKey;
  code: number | null;
  genericKey: GenericKey | null;
};

const cases: Case[] = [
  { input: { code: 101 }, board: 'permission', code: 101, genericKey: null },
  { input: { code: 110 }, board: 'offline', code: 110, genericKey: null },
  { input: { code: 120 }, board: 'noHost', code: 120, genericKey: null },
  { input: { code: 130 }, board: 'policy', code: 130, genericKey: null },
  { input: { code: 131 }, board: 'generic', code: 131, genericKey: 131 },
  { input: { code: 132 }, board: 'generic', code: 132, genericKey: 132 },
  { input: { code: 140 }, board: 'denied', code: 140, genericKey: null },
  // 141 is handled in-session as a toast; reaching this page falls back to 199.
  { input: { code: 141 }, board: 'generic', code: 141, genericKey: 199 },
  { input: { code: 142 }, board: 'generic', code: 142, genericKey: 142 },
  {
    input: { code: 150 },
    board: 'fingerprint',
    code: 150,
    genericKey: null,
  },
  { input: { code: 160 }, board: 'vpn', code: 160, genericKey: null },
  { input: { code: 161 }, board: 'port', code: 161, genericKey: null },
  { input: { code: 170 }, board: 'version', code: 170, genericKey: null },
  { input: { code: 171 }, board: 'appVersion', code: 171, genericKey: null },
  { input: { code: 180 }, board: 'generic', code: 180, genericKey: 180 },
  { input: { code: 181 }, board: 'generic', code: 181, genericKey: 181 },
  {
    input: { code: 190 },
    board: 'loginWindow',
    code: 190,
    genericKey: null,
  },
  { input: { code: 191 }, board: 'fileVault', code: 191, genericKey: null },
  { input: { code: 199 }, board: 'generic', code: 199, genericKey: 199 },
  {
    input: { reason: 'idleTimeout' },
    board: 'generic',
    code: null,
    genericKey: 'idleTimeout',
  },
  {
    input: { reason: 'hostClosedByLocalUser' },
    board: 'generic',
    code: null,
    genericKey: 'hostClosedByLocalUser',
  },
];

describe('classifyEnd', () => {
  it.each(cases)('classifies $input', ({ input, board, code, genericKey }) => {
    expect(classifyEnd(input)).toEqual({ board, code, genericKey });
  });

  it('keeps an unknown code and uses the generic page', () => {
    expect(classifyEnd({ code: 999 })).toEqual({
      board: 'generic',
      code: 999,
      genericKey: 199,
    });
  });

  it('maps a revoked reason to E180 without inventing a code', () => {
    expect(classifyEnd({ reason: 'revoked' })).toEqual({
      board: 'generic',
      code: null,
      genericKey: 180,
    });
  });

  it('maps permissionLost to the permission board', () => {
    expect(classifyEnd({ reason: 'permissionLost' })).toEqual({
      board: 'permission',
      code: null,
      genericKey: null,
    });
  });

  it('maps protocolError to generic E199', () => {
    expect(classifyEnd({ reason: 'protocolError' })).toEqual({
      board: 'generic',
      code: null,
      genericKey: 199,
    });
  });

  it('uses generic E199 with no error code when nothing is known', () => {
    expect(classifyEnd({})).toEqual({
      board: 'generic',
      code: null,
      genericKey: 199,
    });
  });
});
