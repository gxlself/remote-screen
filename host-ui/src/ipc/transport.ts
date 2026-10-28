export interface RpcErrorShape {
  code: number;
  message: string;
  data?: { code?: string } | null;
}

export const AGENT_UNAVAILABLE: RpcErrorShape = {
  code: -32099,
  message: 'agent not connected',
  data: { code: 'agent_unavailable' },
};

export interface Transport {
  call(method: string, params: unknown): Promise<unknown>;
  subscribe(listener: (method: string, params: unknown) => void): () => void;
  onConnection(listener: (connected: boolean) => void): () => void;
  isConnected(): Promise<boolean>;
}
