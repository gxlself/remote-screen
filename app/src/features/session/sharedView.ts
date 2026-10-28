import { sharedRelayTcp } from '../../i18n/zh/sharedRelayTcp';
import type { SessionRecord } from './store';

export type SharedBar =
  | { kind: 'view'; controllerName: string; sameUser: boolean }
  | { kind: 'viewOnly' };

export type SharedView = {
  bar: SharedBar | null;
  watchers: string | null;
};

export function sharedView(i: {
  rec: SessionRecord | undefined;
  myUser: string | null;
  myDeviceId: string | null;
}): SharedView {
  const rec = i.rec;
  if (rec === undefined) {
    return { bar: null, watchers: null };
  }

  let bar: SharedBar | null = null;
  if (rec.role === 'viewer') {
    const controller = rec.controller;
    if (controller !== null && controller.deviceId !== i.myDeviceId) {
      bar = {
        kind: 'view',
        controllerName: controller.deviceName,
        sameUser: controller.user === i.myUser,
      };
    } else {
      bar = { kind: 'viewOnly' };
    }
  }

  let watchers: string | null = null;
  if (rec.role === 'controller' && rec.viewers.length > 0) {
    const first = rec.viewers[0];
    watchers = sharedRelayTcp.shared.watchers(
      first.deviceName,
      rec.viewers.length,
    );
  }

  return { bar, watchers };
}

/** 取设备名的最后一段作为简称，例如「小明的 iPad」→「iPad」。 */
export function shortDeviceName(deviceName: string): string {
  const marker = deviceName.lastIndexOf('的 ');
  return marker < 0 ? deviceName : deviceName.slice(marker + 2);
}
