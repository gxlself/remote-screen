// 网络工作配置的默认透传实现。
import type { NetworkStatus } from '../rscore/types';

export function applyWorkProfile(status: NetworkStatus): NetworkStatus {
  return status;
}

export function subscribeWorkProfile(_cb: () => void): () => void {
  return () => {};
}
