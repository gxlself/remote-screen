import type { CodegenTypes, TurboModule } from 'react-native';
import { TurboModuleRegistry } from 'react-native';

export type PipChangeEvent = {
  /** 当前画中画对应的会话；没有会话时为空串。 */
  sessionId: string;
  /** true：正在进入或已在画中画（willStart 起）；false：已结束或没能进入。 */
  active: boolean;
  /**
   * willStart | started | restore | closed | stopped | failed | notPossible |
   * ineligible | foreground | detached
   */
  reason: string;
};

export type PipPlaybackEvent = {
  sessionId: string;
  paused: boolean;
};

export interface Spec extends TurboModule {
  getConstants(): { supported: boolean };
  /** 资格变化时调；sessionId 为空串表示没有会话。原生在主线程异步应用。 */
  setEligibility(
    sessionId: string,
    canStart: boolean,
    canContinue: boolean,
  ): void;
  /** 手动进入；不满足条件时发 onPipChange{active:false, reason:'notPossible'}。 */
  start(): void;
  /** 手动退出（会话页占位的「回到全屏」）。 */
  stop(): void;
  /** 同步读取：正在进入或已在画中画。 */
  isActive(): boolean;
  readonly onPipChange: CodegenTypes.EventEmitter<PipChangeEvent>;
  readonly onPipPlayback: CodegenTypes.EventEmitter<PipPlaybackEvent>;
}

export default TurboModuleRegistry.get<Spec>('RsPip');
