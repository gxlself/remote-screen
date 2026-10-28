import type { EventName } from './events';
import { APPROVAL_PAIRING, type FakeState } from './fixtures';

export const SCENARIOS = [
  'controlled',
  'idle',
  'handoff',
  'approval',
  'update',
  'update-ready',
  'setup',
  'offline',
] as const;

export type ScenarioName = (typeof SCENARIOS)[number];

interface ScenarioTarget {
  readonly state: FakeState;
  readonly nowMs: number;
  emit(method: EventName, params: unknown): void;
  setConnected(connected: boolean): void;
}

function isScenarioName(value: string | null): value is ScenarioName {
  return value !== null && (SCENARIOS as readonly string[]).includes(value);
}

export function applyScenario(fake: ScenarioTarget, name: string | null): void {
  const scenario: ScenarioName = isScenarioName(name) ? name : 'controlled';

  switch (scenario) {
    case 'controlled':
      break;
    case 'idle':
      fake.state.sessions = [];
      break;
    case 'handoff':
      {
        fake.state.sessions.push({
          session_id: 's-2',
          device_id: 'm4ntq8z2c7v5xa1d',
          device_name: '小明的 iPad',
          role: 'viewer',
          connected_at_us: (fake.nowMs - 600_000) * 1000,
        });
      }
      break;
    case 'approval':
      fake.state.sessions = [];
      setTimeout(() => {
        fake.emit('event.approvalRequested', APPROVAL_PAIRING);
      }, 300);
      break;
    case 'update':
      fake.state.update = {
        state: 'waiting_session_end',
        version: '0.3.2',
        notes: '修复：TCP 模式下偶发花屏\n新增：「内网连接每次确认」',
      };
      break;
    case 'update-ready':
      fake.state.update = {
        state: 'idle',
        version: '0.3.2',
        notes: '修复：TCP 模式下偶发花屏\n新增：「内网连接每次确认」',
      };
      fake.state.sessions = [];
      break;
    case 'setup':
      fake.state.status.permissions = {
        screen_recording: false,
        accessibility: false,
      };
      fake.state.devices = [];
      fake.state.sessions = [];
      break;
    case 'offline':
      fake.setConnected(false);
      break;
  }
}
