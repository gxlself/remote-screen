import type { BoardKey } from '../../features/errors/classify';
import type { BoardComponent } from './boardTypes';
import DeniedBoard from './boards/DeniedBoard';
import FingerprintBoard from './boards/FingerprintBoard';
import GenericBoard from './boards/GenericBoard';
import LoginWindowBoard from './boards/LoginWindowBoard';
import NoHostBoard from './boards/NoHostBoard';
import OfflineBoard from './boards/OfflineBoard';
import PermissionBoard from './boards/PermissionBoard';
import PolicyBoard from './boards/PolicyBoard';
import PortBoard from './boards/PortBoard';
import VersionBoard from './boards/VersionBoard';
import VpnBoard from './boards/VpnBoard';

const BOARDS: Record<BoardKey, BoardComponent> = {
  permission: PermissionBoard,
  offline: OfflineBoard,
  noHost: NoHostBoard,
  policy: PolicyBoard,
  denied: DeniedBoard,
  fingerprint: FingerprintBoard,
  vpn: VpnBoard,
  port: PortBoard,
  version: VersionBoard,
  appVersion: VersionBoard,
  loginWindow: LoginWindowBoard,
  fileVault: LoginWindowBoard,
  generic: GenericBoard,
};

export function boardFor(key: BoardKey): BoardComponent {
  return BOARDS[key];
}
