import type { CodegenTypes, TurboModule } from 'react-native';
import { TurboModuleRegistry } from 'react-native';

export type WindowStateEvent = {
  /** `main`, or the id JS gave the window when it asked for it. */
  windowId: string;
  /** active | inactive | background | closed */
  state: string;
};

export interface Spec extends TurboModule {
  /** supported：iPad 且 Info.plist 允许多个窗口。 */
  getConstants(): { supported: boolean };
  /** 开一个新的系统窗口；payload 原样交回给窗口的根组件。失败时发 closed。 */
  open(windowId: string, payload: string, title: string): void;
  /** 把这个窗口带到前面；main 没有窗口时新开一个主窗口。 */
  activate(windowId: string): void;
  /** 关掉这个窗口（requestSceneSessionDestruction）。 */
  close(windowId: string): void;
  /** 把每个已知窗口的当前状态再发一遍。 */
  refresh(): void;
  readonly onWindowState: CodegenTypes.EventEmitter<WindowStateEvent>;
}

export default TurboModuleRegistry.get<Spec>('RsWindow');
