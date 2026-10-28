import type { TurboModule } from 'react-native';
import { TurboModuleRegistry } from 'react-native';

export type NativePickedFile = { uri: string; name: string; size: number }; // size -1 = 未知

export interface Spec extends TurboModule {
  getConstants(): { filesDir: string; inboxDir: string };
  // UIDocumentPicker（asCopy），多选；拷进 inboxDir 后返回 file:// URI。取消 → []
  pickFiles(): Promise<NativePickedFile[]>;
  // UIDocumentPicker forExporting。true = 已保存，false = 用户取消
  saveToFiles(uri: string, name: string): Promise<boolean>;
  // 其他 App「用 Remote Screen 打开」/ 分享进来的 file:// URL：拷进 inboxDir；不是文件 URL → null
  importIncoming(url: string): Promise<NativePickedFile | null>;
  // 有传输在跑时 true：申请后台执行时间；全部结束时 false
  setTransfersActive(active: boolean): void;
}

export default TurboModuleRegistry.get<Spec>('RsFiles');
