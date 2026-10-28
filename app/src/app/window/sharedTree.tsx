import { useContext, useEffect } from 'react';
import type { ReactNode } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { create } from 'zustand';
import {
  EventsValueProvider,
  useEventsValue,
} from '../../features/events/EventsProvider';
import type { EventsValue } from '../../features/events/EventsProvider';
import { FilesProvider, useFiles } from '../../features/files/FilesContext';
import type { FilesContextValue } from '../../features/files/FilesContext';
import { ServicesProviders } from '../../features/data/ServicesProviders';
import type { AppServices } from '../../features/data/appServices';
import {
  SshStoreContext,
  SshStoreProvider,
} from '../../features/sessions/sshStore';
import type { SshStoreApi } from '../../features/sessions/sshStore';
import { useSettingsStore } from '../../features/settings/store';
import { useKv } from '../../features/storage/StorageContext';
import { usePlatform } from '../../native/platform/PlatformContext';
import { useCore } from '../../native/rscore/CoreContext';

/**
 * a window of its own is a second React root in the same JS
 * runtime. It uses the main tree's Core, settings, stores and event bus, so
 * a session behaves the same whichever window shows it.
 */
export type SharedTree = {
  services: AppServices;
  events: EventsValue;
  files: FilesContextValue;
  /** Tests scope the SSH store; the app uses the module-wide one. */
  ssh: SshStoreApi | null;
};

export const sharedTree = create<{ tree: SharedTree | null }>(() => ({
  tree: null,
}));

/** Mounted inside the main tree's providers, once they are ready. */
export function PublishSharedTree(): null {
  const core = useCore();
  const platform = usePlatform();
  const kv = useKv();
  const settingsStore = useSettingsStore();
  const queryClient = useQueryClient();
  const events = useEventsValue();
  const files = useFiles();
  const ssh = useContext(SshStoreContext);
  useEffect(() => {
    if (events === null) {
      return;
    }
    sharedTree.setState({
      tree: {
        services: { core, platform, kv, settingsStore, queryClient },
        events,
        files,
        ssh,
      },
    });
  }, [core, events, files, kv, platform, queryClient, settingsStore, ssh]);
  // The main tree stays for the app's lifetime; a remount publishes again.
  return null;
}

export function useSharedTree(): SharedTree | null {
  return sharedTree(state => state.tree);
}

export function SharedProviders({
  tree,
  children,
}: {
  tree: SharedTree;
  children: ReactNode;
}) {
  let content = children;
  if (tree.ssh !== null) {
    content = <SshStoreProvider store={tree.ssh}>{content}</SshStoreProvider>;
  }
  if (tree.files.available) {
    content = (
      <FilesProvider files={tree.files.files} picker={tree.files.picker}>
        {content}
      </FilesProvider>
    );
  }
  return (
    <ServicesProviders {...tree.services}>
      <EventsValueProvider value={tree.events}>{content}</EventsValueProvider>
    </ServicesProviders>
  );
}
