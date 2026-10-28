export type BoardKey =
  | 'permission'
  | 'offline'
  | 'noHost'
  | 'policy'
  | 'denied'
  | 'fingerprint'
  | 'vpn'
  | 'port'
  | 'version'
  | 'appVersion'
  | 'loginWindow'
  | 'fileVault'
  | 'generic';

export type GenericKey =
  | 131
  | 132
  | 142
  | 180
  | 181
  | 199
  | 'idleTimeout'
  | 'hostClosedByLocalUser';

export type EndClassification = {
  board: BoardKey;
  code: number | null;
  genericKey: GenericKey | null;
};

const CODE_BOARDS: Readonly<Record<number, BoardKey>> = {
  101: 'permission',
  110: 'offline',
  120: 'noHost',
  130: 'policy',
  131: 'generic',
  132: 'generic',
  140: 'denied',
  141: 'generic',
  142: 'generic',
  150: 'fingerprint',
  160: 'vpn',
  161: 'port',
  170: 'version',
  171: 'appVersion',
  180: 'generic',
  181: 'generic',
  190: 'loginWindow',
  191: 'fileVault',
  199: 'generic',
};

const GENERIC_CODES: readonly number[] = [131, 132, 142, 180, 181, 199];

const REASON_GENERIC: Readonly<Record<string, GenericKey>> = {
  idleTimeout: 'idleTimeout',
  hostClosedByLocalUser: 'hostClosedByLocalUser',
  revoked: 180,
  protocolError: 199,
};

const REASON_BOARD: Readonly<Record<string, BoardKey>> = {
  revoked: 'generic',
  permissionLost: 'permission',
  protocolError: 'generic',
};

function genericKeyForCode(code: number): GenericKey {
  return GENERIC_CODES.includes(code) ? (code as GenericKey) : 199;
}

export function classifyEnd(p: {
  code?: number;
  reason?: string;
}): EndClassification {
  if (p.code !== undefined) {
    const board = CODE_BOARDS[p.code] ?? 'generic';
    return {
      board,
      code: p.code,
      genericKey: board === 'generic' ? genericKeyForCode(p.code) : null,
    };
  }

  if (p.reason !== undefined) {
    const genericKey = REASON_GENERIC[p.reason];
    if (genericKey !== undefined) {
      return { board: 'generic', code: null, genericKey };
    }
    const board = REASON_BOARD[p.reason];
    if (board !== undefined) {
      return {
        board,
        code: null,
        genericKey: board === 'generic' ? 199 : null,
      };
    }
  }

  return { board: 'generic', code: null, genericKey: 199 };
}
