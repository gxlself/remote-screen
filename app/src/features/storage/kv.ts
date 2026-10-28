export interface KvStore {
  getString(k: string): string | undefined;
  set(k: string, v: string): void;
  delete(k: string): void;
  getJSON<T>(k: string): T | undefined;
  setJSON(k: string, v: unknown): void;
  keys(): string[];
}

export class MemoryKv implements KvStore {
  private readonly values: Map<string, string>;

  constructor(init?: Record<string, string>) {
    this.values = new Map(Object.entries(init ?? {}));
  }

  getString(k: string): string | undefined {
    return this.values.get(k);
  }

  set(k: string, v: string): void {
    this.values.set(k, v);
  }

  delete(k: string): void {
    this.values.delete(k);
  }

  getJSON<T>(k: string): T | undefined {
    const raw = this.getString(k);
    if (raw === undefined) {
      return undefined;
    }
    try {
      return JSON.parse(raw) as T;
    } catch {
      return undefined;
    }
  }

  setJSON(k: string, v: unknown): void {
    this.set(k, JSON.stringify(v));
  }

  keys(): string[] {
    return Array.from(this.values.keys());
  }
}
