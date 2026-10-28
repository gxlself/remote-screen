import type { RsCoreApi } from '../../native/rscore/RsCoreApi';
import { isUnsupported, type FilePicker } from '../files/filesApi';
import { createNativePicker } from '../files/nativePicker';

export type ExportOutcome = 'saved' | 'cancelled' | 'unsupported';

/** 'file://' 加上按 '/' 分段 encodeURIComponent 的绝对路径。 */
export function pathToFileUri(path: string): string {
  return `file://${path.split('/').map(encodeURIComponent).join('/')}`;
}

function basename(path: string): string {
  return path.slice(path.lastIndexOf('/') + 1);
}

/**
 * 导出 MP4：Rust 解密封装成临时文件 → 交给系统保存（iOS「文件」App，Android「保存到」）→ 删除临时文件。
 * busy、diskLow、damaged 等错误原样抛出，由调用方按错误码显示文案。
 */
export async function exportRecording(
  core: RsCoreApi,
  recordingId: string,
  picker: FilePicker = createNativePicker(),
): Promise<ExportOutcome> {
  const recording = core.recording;
  if (recording === undefined) {
    return 'unsupported';
  }
  let path: string;
  try {
    path = (await recording.exportMp4(recordingId)).path;
  } catch (error) {
    if (isUnsupported(error)) {
      return 'unsupported';
    }
    throw error;
  }
  try {
    const saved = await picker.saveToFiles(pathToFileUri(path), basename(path));
    return saved ? 'saved' : 'cancelled';
  } catch (error) {
    if (isUnsupported(error)) {
      return 'unsupported';
    }
    throw error;
  } finally {
    try {
      recording.discardExport(path);
    } catch {
      // 删不掉的临时文件在下一次导出前清理。
    }
  }
}
