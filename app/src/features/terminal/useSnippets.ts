import { useCallback, useMemo } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { useCore } from '../../native/rscore/CoreContext';
import { useSync } from '../data/queries';
import { qk } from '../data/queryClient';
import { putSyncWithRetry } from '../data/syncRetry';
import type { Snippet, SnippetsValue } from './snippets';
import {
  addSnippet,
  parseSnippets,
  removeSnippet,
  updateSnippet,
} from './snippets';

function newSnippetId(): string {
  return (
    'sn-' +
    Date.now().toString(36) +
    Math.random().toString(36).slice(2, 6).padEnd(4, '0')
  );
}

export function useSnippets(): {
  items: Snippet[];
  isLoading: boolean;
  // Reading this phone's copy failed: not the same as having none.
  isError: boolean;
  retry(): void;
  add(snippet: Omit<Snippet, 'id'>): Promise<void>;
  update(snippet: Snippet): Promise<void>;
  remove(id: string): Promise<void>;
} {
  const core = useCore();
  const queryClient = useQueryClient();
  const sync = useSync<unknown>('snippets');
  const value = useMemo(() => parseSnippets(sync.data?.value), [sync.data]);

  const write = useCallback(
    async (
      update: (current: SnippetsValue) => SnippetsValue,
    ): Promise<void> => {
      const result = await putSyncWithRetry<SnippetsValue>(
        core,
        'snippets',
        current => update(parseSnippets(current)),
      );
      queryClient.setQueryData(qk.sync('snippets'), result);
    },
    [core, queryClient],
  );

  const add = useCallback(
    (snippet: Omit<Snippet, 'id'>) => {
      const id = newSnippetId();
      return write(current => addSnippet(current, snippet, id));
    },
    [write],
  );
  const update = useCallback(
    (snippet: Snippet) => write(current => updateSnippet(current, snippet)),
    [write],
  );
  const remove = useCallback(
    (id: string) => write(current => removeSnippet(current, id)),
    [write],
  );

  return {
    items: value.items,
    isLoading: sync.isLoading,
    isError: sync.isError,
    retry: () => {
      sync.refetch().catch(() => undefined);
    },
    add,
    update,
    remove,
  };
}
