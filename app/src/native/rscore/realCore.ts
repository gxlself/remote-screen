import type { RsCoreApi } from './RsCoreApi';
import type { RsMobileBindings } from './bindings';
import { createRsCore } from './createRsCore';

/** 真实 RsCore：只在原生环境调用（jest 里 react-native-rs-core 被 mock 成空对象）。 */
export function loadRealCore(): RsCoreApi {
  const native = require('react-native-rs-core') as typeof import('react-native-rs-core');
  const bindings: RsMobileBindings = native; // 编译期核对生成签名
  return createRsCore(bindings);
}
