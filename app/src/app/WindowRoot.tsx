import { useEffect, useState } from 'react';
import { ActivityIndicator, StatusBar, StyleSheet, View } from 'react-native';
import { SafeAreaProvider } from 'react-native-safe-area-context';
import AppText from '../components/AppText';
import { Button } from '../components/Button';
import { adapterLive } from '../features/adapters/liveStore';
import { useLockState } from '../features/lock/lockState';
import {
  WindowProvider,
  useWindowMeasured,
} from '../features/session/WindowContext';
import { leaveWindow, MAIN_WINDOW } from '../features/session/windows';
import { decodeWindowPayload } from '../features/session/windowTarget';
import type { WindowPayload } from '../features/session/windowTarget';
import { windows as copy } from '../i18n/zh/windows';
import { activateWindow } from '../native/window/window';
import { space, tokens } from '../theme/tokens';
import { SharedProviders, useSharedTree } from './window/sharedTree';
import type { SharedTree } from './window/sharedTree';
import WindowNavigator from './window/WindowNavigator';

export type WindowRootProps = {
  windowId?: unknown;
  /** The session (or 选电脑) to show, from the scene's activity (windowTarget.ts). */
  payload?: unknown;
};

/** Whether the session is still there to show (not after a relaunch). */
function targetLive(target: WindowPayload, tree: SharedTree): boolean {
  if (target.kind === 'picker') {
    return true;
  }
  if (target.kind === 'host') {
    const record = tree.events.sessionsStore.getState()[target.sessionId];
    return record !== undefined && record.phase !== 'ended';
  }
  // Folded away by the page it came from, or never shown since.
  return adapterLive.peek(target.sessionId) !== undefined;
}

function Black({ loading = false }: { loading?: boolean }) {
  return (
    <View style={styles.center} testID="window-waiting">
      {loading ? (
        <ActivityIndicator
          accessibilityLabel={copy.loading}
          color={tokens.text2}
        />
      ) : null}
    </View>
  );
}

function LockedCover() {
  return (
    <View style={[StyleSheet.absoluteFill, styles.center]} testID="window-locked">
      <View style={styles.lockedContent}>
        <AppText accessibilityRole="header" center variant="headline">
          {copy.locked.title}
        </AppText>
        <AppText center color="text2" variant="footnote">
          {copy.locked.body}
        </AppText>
        <Button
          onPress={() => activateWindow(MAIN_WINDOW)}
          testID="window-locked-main"
          title={copy.locked.action}
        />
      </View>
    </View>
  );
}

function WindowContent({
  windowId,
  target,
  tree,
}: {
  windowId: string;
  target: WindowPayload;
  tree: SharedTree;
}) {
  // Checked once: the page owns the session from then on. A window iPadOS
  // brings back after a relaunch finds nothing and closes. A 选电脑 window
  // always has its list.
  const [live] = useState(() => targetLive(target, tree));
  const locked = useLockState(state => state.locked);
  // The page opens its connection at this window's size, not the key one's.
  const measured = useWindowMeasured();
  useEffect(() => {
    if (!live) {
      leaveWindow(windowId);
    }
  }, [live, windowId]);
  if (!live || !measured) {
    return <Black />;
  }
  return (
    <SharedProviders tree={tree}>
      <WindowNavigator payload={target} />
      {/* App lock is asked for in the main window; meanwhile cover this one. */}
      {locked ? <LockedCover /> : null}
    </SharedProviders>
  );
}

/**
 * the root of a window of its own, registered as
 * `RemoteScreenWindow`. It shares the main window's JS runtime and stores,
 * opens straight on its session's page and closes when that page is left.
 * A window iPadOS opened by itself starts on the computer list instead and
 * stays open until the user closes it.
 * App-wide pieces (session lifecycle, history, deep links, app lock, native
 * bridges) live in the main window only, which is always mounted.
 */
export default function WindowRoot({ windowId, payload }: WindowRootProps) {
  const tree = useSharedTree();
  const [target] = useState(() => decodeWindowPayload(payload));
  const id =
    typeof windowId === 'string' && windowId !== '' && windowId !== MAIN_WINDOW
      ? windowId
      : null;
  useEffect(() => {
    if (id !== null && target === null) {
      leaveWindow(id);
    }
  }, [id, target]);

  let content;
  if (id === null || target === null) {
    content = <Black />;
  } else if (tree === null) {
    content = <Black loading />;
  } else {
    content = <WindowContent target={target} tree={tree} windowId={id} />;
  }
  return (
    <SafeAreaProvider>
      <StatusBar barStyle="light-content" />
      <WindowProvider windowId={id ?? ''}>{content}</WindowProvider>
    </SafeAreaProvider>
  );
}

const styles = StyleSheet.create({
  center: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: '#000',
  },
  lockedContent: {
    alignItems: 'center',
    gap: space.lg,
    maxWidth: 360,
    padding: space.gutter,
  },
});
