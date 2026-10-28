import { useEffect, useMemo } from 'react';
import { NavigationContainer, useIsFocused } from '@react-navigation/native';
import type { NavigationAction } from '@react-navigation/native';
import { createNativeStackNavigator } from '@react-navigation/native-stack';
import { StyleSheet, View } from 'react-native';
import AdapterSessionScreen from '../../features/adapters/AdapterSessionScreen';
import { useWindowId } from '../../features/session/WindowContext';
import { leaveWindow, MAIN_WINDOW } from '../../features/session/windows';
import { windowRoute } from '../../features/session/windowTarget';
import type { WindowPayload } from '../../features/session/windowTarget';
import { activateWindow } from '../../native/window/window';
import SessionScreen from '../../screens/Session';
import SessionEndedScreen from '../../screens/SessionEnded';
import { navTheme } from '../navigation/navTheme';
import { navigationRef } from '../navigation/navigationRef';
import type { RootStackParamList } from '../navigation/routes';
import PickerScreen from './PickerScreen';

const Stack = createNativeStackNavigator<RootStackParamList>();

const immersive = {
  autoHideHomeIndicator: true,
  navigationBarHidden: true,
} as const;

/**
 * Stands in for the main window's tabs under the session page. Whatever
 * takes the page back there (返回、收起、连接其他电脑、a clean end) lands
 * here, and the window closes with the main window brought forward.
 */
function WindowExit() {
  const windowId = useWindowId();
  const focused = useIsFocused();
  useEffect(() => {
    if (focused) {
      leaveWindow(windowId);
    }
  }, [focused, windowId]);
  return <View style={styles.black} testID="window-exit" />;
}

/**
 * The session pages of a window of its own. The page is the same
 * as in the main window; a screen this stack does not have (终端、配对、
 * 诊断) opens in the main window instead. Under the page is either the
 * window's exit (a window for one session) or its computer list (选电脑).
 */
export default function WindowNavigator({
  payload,
}: {
  payload: WindowPayload;
}) {
  const picker = payload.kind === 'picker';
  const initialState = useMemo(() => {
    if (payload.kind === 'picker') {
      return undefined;
    }
    const route = windowRoute(payload);
    return {
      index: 1,
      routes: [{ name: 'MainTabs' as const }, route],
    };
  }, [payload]);
  const handOff = (action: NavigationAction) => {
    if (navigationRef.isReady()) {
      navigationRef.dispatch(action);
    }
    activateWindow(MAIN_WINDOW);
  };
  return (
    <NavigationContainer
      {...(initialState === undefined ? {} : { initialState })}
      onUnhandledAction={handOff}
      theme={navTheme}
    >
      <Stack.Navigator
        screenOptions={{
          headerShown: false,
          contentStyle: { backgroundColor: '#000' },
        }}
      >
        <Stack.Screen
          component={picker ? PickerScreen : WindowExit}
          name="MainTabs"
        />
        <Stack.Group
          screenOptions={{
            presentation: 'fullScreenModal',
            statusBarHidden: true,
            gestureEnabled: false,
            animation: 'fade',
          }}
        >
          <Stack.Screen
            component={SessionScreen}
            name="Session"
            options={immersive}
          />
          <Stack.Screen component={SessionEndedScreen} name="SessionEnded" />
          <Stack.Screen
            component={AdapterSessionScreen}
            name="AdapterSession"
            options={immersive}
          />
        </Stack.Group>
      </Stack.Navigator>
    </NavigationContainer>
  );
}

const styles = StyleSheet.create({
  black: { flex: 1, backgroundColor: '#000' },
});
