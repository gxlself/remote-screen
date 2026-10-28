import { NavigationContainer } from '@react-navigation/native';
import { StatusBar } from 'react-native';
import ToastHost from '../components/ToastHost';
import DeepLinkPairing from '../features/deeplink/DeepLinkPairing';
import IncomingFiles from '../features/files/IncomingFiles';
import IdentityBootstrap from '../features/identity/IdentityBootstrap';
import IncomingPairLinks from '../features/pairing/IncomingPairLinks';
import SessionLifecycle from '../features/session/SessionLifecycle';
import { WindowProvider } from '../features/session/WindowContext';
import WindowLifecycle from '../features/session/WindowLifecycle';
import { MAIN_WINDOW } from '../features/session/windows';
import HistoryRecorder from '../features/sessions/HistoryRecorder';
import NetworksSync from '../features/network/NetworksSync';
import AppLockGate from '../screens/AppLockGate';
import AppProviders from './AppProviders';
import NativeBridges from './NativeBridges';
import RootNavigator from './navigation/RootNavigator';
import { navTheme } from './navigation/navTheme';
import { navigationRef } from './navigation/navigationRef';
import { PublishSharedTree } from './window/sharedTree';

export default function AppRoot() {
  return (
    <WindowProvider windowId={MAIN_WINDOW}>
      <AppProviders>
        {/* Every screen is dark; the session route hides the bar itself. */}
        <StatusBar barStyle="light-content" />
        <NavigationContainer ref={navigationRef} theme={navTheme}>
          <RootNavigator />
          <IncomingPairLinks />
        </NavigationContainer>
        <ToastHost />
        <IdentityBootstrap />
        <DeepLinkPairing />
        <IncomingFiles />
        <SessionLifecycle />
        <HistoryRecorder />
        <NetworksSync />
        <AppLockGate />
        <NativeBridges />
        {/* windows of their own use this tree's services and stores. */}
        <WindowLifecycle />
        <PublishSharedTree />
      </AppProviders>
    </WindowProvider>
  );
}
