import type { HostOs, Platform, TrayState, WindowLabel } from './types';

export interface PlatformCall {
  op: keyof Platform;
  args: unknown[];
}

export class FakePlatform implements Platform {
  readonly os: HostOs;
  readonly calls: PlatformCall[] = [];
  clipboard: string | null = null;

  constructor(options: { os?: HostOs } = {}) {
    this.os = options.os ?? 'macos';
  }

  private record(op: keyof Platform, args: unknown[]): void {
    this.calls.push({ op, args });
  }

  async showWindow(label: WindowLabel, route?: string): Promise<void> {
    this.record('showWindow', [label, route]);
  }

  async hideWindow(label: WindowLabel): Promise<void> {
    this.record('hideWindow', [label]);
  }

  async setTrayState(state: TrayState): Promise<void> {
    this.record('setTrayState', [state]);
  }

  async notify(title: string, body: string): Promise<void> {
    this.record('notify', [title, body]);
  }

  async openUrl(url: string): Promise<void> {
    this.record('openUrl', [url]);
  }

  async revealPath(path: string): Promise<void> {
    this.record('revealPath', [path]);
  }

  async copyText(text: string): Promise<void> {
    this.clipboard = text;
    this.record('copyText', [text]);
  }

  async quit(): Promise<void> {
    this.record('quit', []);
  }
}
