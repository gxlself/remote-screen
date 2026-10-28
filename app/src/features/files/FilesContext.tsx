import { createContext, useContext } from 'react';
import type { JSX, ReactNode } from 'react';
import {
  UNSUPPORTED_FILES,
  UNSUPPORTED_PICKER,
  type FilePicker,
  type FilesApi,
} from './filesApi';

export type FilesContextValue = {
  files: FilesApi;
  picker: FilePicker;
  available: boolean;
};

const UNAVAILABLE_FILES: FilesContextValue = {
  files: UNSUPPORTED_FILES,
  picker: UNSUPPORTED_PICKER,
  available: false,
};

const FilesContext = createContext<FilesContextValue | null>(null);

export function FilesProvider({
  files,
  picker,
  children,
}: {
  files: FilesApi;
  picker: FilePicker;
  children: ReactNode;
}): JSX.Element {
  return (
    <FilesContext.Provider value={{ files, picker, available: true }}>
      {children}
    </FilesContext.Provider>
  );
}

export function useFiles(): FilesContextValue {
  return useContext(FilesContext) ?? UNAVAILABLE_FILES;
}
