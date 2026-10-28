import NativeRsFiles from '../../native/specs/NativeRsFiles';
import type { Spec as NativeRsFilesSpec } from '../../native/specs/NativeRsFiles';
import { UNSUPPORTED_PICKER, type FilePicker } from './filesApi';

export function createNativePicker(
  mod: NativeRsFilesSpec | null | undefined = NativeRsFiles,
): FilePicker {
  if (mod === null || mod === undefined) {
    return UNSUPPORTED_PICKER;
  }
  return {
    pickFiles: async () => {
      const files = await mod.pickFiles();
      return files.map(file => ({
        uri: file.uri,
        name: file.name,
        size: file.size < 0 ? null : file.size,
      }));
    },
    saveToFiles: (uri, name) => mod.saveToFiles(uri, name),
  };
}
