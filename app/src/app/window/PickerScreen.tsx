import { useMemo, useRef, useState } from 'react';
import { CommonActions } from '@react-navigation/native';
import { FallbackGuide } from '../../features/fallback/FallbackGuide';
import { useWindowId, useWindowSize } from '../../features/session/WindowContext';
import {
  MAIN_WINDOW,
  otherWindowShowing,
  windows,
} from '../../features/session/windows';
import { activateWindow } from '../../native/window/window';
import { HomeNarrow } from '../../screens/Home/HomeNarrow';
import { useHomeModel } from '../../screens/Home/useHomeModel';
import type { ScrollViewHandle } from '../../screens/Home/useTabScrollToTop';
import { HomeWide } from '../../screens/Home/wide/HomeWide';
import { ConnectNavigatorContext } from '../navigation/connectNavigator';
import type {
  ConnectNavigator,
  ConnectRoute,
} from '../navigation/connectNavigator';
import { navigationRef } from '../navigation/navigationRef';
import type {
  RootScreenProps,
  RootStackParamList,
  TabScreenProps,
} from '../navigation/routes';

type WindowNavigation = RootScreenProps<'MainTabs'>['navigation'];
type HomeNavigation = TabScreenProps<'Home'>['navigation'];

/** The pages this window's own stack has (WindowNavigator). */
const HERE = new Set<string>(['Session', 'SessionEnded', 'AdapterSession']);

/** The window that already shows the session a page would open, if any. */
function shownElsewhere(
  windowId: string,
  name: string,
  params: unknown,
): string | null {
  const state = windows.getState();
  if (name === 'Session') {
    const p = params as RootStackParamList['Session'] | undefined;
    return otherWindowShowing(state, windowId, p?.sessionId);
  }
  if (name === 'AdapterSession') {
    const p = params as RootStackParamList['AdapterSession'] | undefined;
    return otherWindowShowing(state, windowId, p?.sessionId, p?.target);
  }
  return null;
}

/** 添加电脑、设置、终端……: the main window has them. */
function openInMain(name: string, params: unknown): void {
  if (navigationRef.isReady()) {
    navigationRef.dispatch(
      CommonActions.navigate(name, params as object | undefined),
    );
  }
  activateWindow(MAIN_WINDOW);
}

/**
 * Opens `name` here or in the main window, or brings forward the window
 * that already shows its session.
 */
function route(
  windowId: string,
  name: string,
  params: unknown,
  here: () => void,
): void {
  const shown = shownElsewhere(windowId, name, params);
  if (shown !== null) {
    activateWindow(shown);
  } else if (HERE.has(name)) {
    here();
  } else {
    openInMain(name, params);
  }
}

function PickerHome({ navigation }: { navigation: HomeNavigation }) {
  const [query, setQuery] = useState('');
  const { width } = useWindowSize();
  const scrollRef = useRef<ScrollViewHandle>(null);
  const { guide, closeGuide, ...model } = useHomeModel(query, navigation);
  const props = { ...model, query, setQuery, scrollRef };
  return (
    <>
      {width >= 700 ? <HomeWide {...props} /> : <HomeNarrow {...props} />}
      <FallbackGuide guide={guide} onClose={closeGuide} />
    </>
  );
}

/**
 * the computer list of a window iPadOS opened by itself (选电脑).
 * A computer opens here, full screen; leaving its page comes back to this
 * list, and the window stays until the user closes it. A computer another
 * window shows brings that window forward instead.
 */
export default function PickerScreen({
  navigation,
}: {
  navigation: WindowNavigation;
}) {
  const windowId = useWindowId();
  const connectNavigator = useMemo<ConnectNavigator>(() => {
    const open = (name: ConnectRoute, params: object) =>
      route(windowId, name, params, () =>
        navigation.dispatch(CommonActions.navigate(name, params)),
      );
    // Nothing here to replace: the list stays under the page.
    return { navigate: open, replace: open };
  }, [navigation, windowId]);
  const homeNavigation = useMemo(() => {
    const navigate = (name: string, params?: unknown) =>
      route(windowId, name, params, () =>
        navigation.dispatch(
          CommonActions.navigate(name, params as object | undefined),
        ),
      );
    return { ...navigation, navigate } as unknown as HomeNavigation;
  }, [navigation, windowId]);
  return (
    <ConnectNavigatorContext.Provider value={connectNavigator}>
      <PickerHome navigation={homeNavigation} />
    </ConnectNavigatorContext.Provider>
  );
}
