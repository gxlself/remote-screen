import { zh as zhBase } from './zh';
import type { HostOs } from '../platform/types';

type DeepPartial<T> = T extends (...args: never[]) => unknown
  ? T
  : T extends string
    ? string
    : T extends number
      ? number
      : T extends boolean
        ? boolean
        : T extends object
          ? { readonly [Key in keyof T]?: DeepPartial<T[Key]> }
          : T;

export const WIN_OVERRIDES = {
  common: {
    thisMac: '这台电脑',
  },
  menu: {
    online: '服务运行中',
    offline: '服务没有运行',
    quitShortcut: '',
  },
  capsule: {
    controlling: (name: string) => `${name} 正在控制此电脑`,
    watchingOne: (name: string) => `${name} 正在观看此电脑`,
    watchingMany: (count: number) => `${count} 台设备正在观看此电脑`,
    recordingA11y: (who: string | number) =>
      typeof who === 'number' ? `${who} 台设备正在录制此电脑的画面` : `${who} 正在录制此电脑的画面`,
  },
  confirm: {
    title: {
      connect: (device: string) => `「${device}」想要连接这台电脑`,
    },
    description: {
      takeover: (controller: string | null) =>
        controller === null
          ? '允许后它开始控制这台电脑。'
          : `现在由 ${controller} 控制；允许后它改为观看。`,
      attended: '这台电脑设成了每次连接都要确认。',
      localActivity: '你刚刚在用这台电脑，所以先问你一下。',
      intranet: '这次是从公司内网连进来的，不经过 tailnet。确认是你本人再允许。',
    },
  },
  pair: {
    status: {
      disabledOnPath: '这台电脑没有允许经内网配对',
    },
  },
  devices: {
    permission: {
      control: {
        description: '每次连进来，这台电脑上都会弹窗，你点允许才开始',
      },
    },
  },
  settings: {
    shell: {
      thisMac: '这台电脑',
    },
    general: {
      access: {
        attendedModeHint: '每次连接都要在这台电脑上点允许',
        emptyUsers: '只允许这台电脑上登录 Tailscale 的账号',
      },
      clipboard: {
        toPhone: '允许把这台电脑的剪贴板发到手机',
      },
    },
  },
  permissions: {
    items: {
      accessibility: {
        description: '把手机上的点按和键盘输入转给这台电脑',
      },
    },
    needsRestart: '授权后 Windows 可能会要求重新打开 Remote Host',
  },
  history: {
    description: '只保存在这台电脑上',
  },
  network: {
    intro: '手机能从哪几条路连到这台电脑',
    intranet: {
      firewallActive: '已添加 Windows 防火墙规则（域、专用网络）',
      firewallOff: 'Windows 防火墙没有打开，不需要规则',
    },
    alerts: {
      noRoutes: '两条路都关了，手机现在连不上这台电脑。至少打开一个。',
    },
  },
  pairingMethods: {
    password: {
      switchLabel: '允许用 Windows 账户密码配对',
      hint: '适合没人能看配对码的时候，错 5 次锁 15 分钟',
    },
  },
  update: {
    session: {
      controlling: (name: string) => `${name} 正在控制这台电脑`,
      watching: (name: string) => `${name} 正在观看这台电脑`,
    },
  },
  bg: {
    watchBody: (controller: string, viewer: string) =>
      `${controller} 仍在控制这台电脑。${viewer} 上点「接管控制」就能换过来。`,
  },
} satisfies DeepPartial<typeof zhBase>;

type TranslationRecord = Record<string, unknown>;

function isRecord(value: unknown): value is TranslationRecord {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function merge(base: unknown, override: unknown): unknown {
  if (override === undefined) {
    return base;
  }
  if (!isRecord(base) || !isRecord(override)) {
    return override;
  }

  const merged: TranslationRecord = { ...base };
  for (const [key, value] of Object.entries(override)) {
    merged[key] = merge(base[key], value);
  }
  return merged;
}

export function zhFor(os: HostOs): typeof zhBase {
  if (os === 'macos') {
    return zhBase;
  }
  return merge(zhBase, WIN_OVERRIDES) as typeof zhBase;
}
