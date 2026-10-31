import { createNativeStackNavigator } from '@react-navigation/native-stack';
import { tokens } from '../../theme/tokens';
import OnboardTailnetScreen from '../../screens/OnboardTailnet';
import OnboardDiscoverScreen from '../../screens/OnboardDiscover';
import OnboardProtectScreen from '../../screens/OnboardProtect';
import DeviceSheetScreen from '../../screens/DeviceSheet';
import DeviceRoutesScreen from '../../screens/DeviceRoutes';
import PairAddScreen from '../../screens/PairAdd';
import PairWaitingScreen from '../../screens/PairWaiting';
import PairDoneScreen from '../../screens/PairDone';
import PairFileScreen from '../../screens/PairFile';
import SessionScreen from '../../screens/Session';
import SessionEndedScreen from '../../screens/SessionEnded';
import AdapterSessionScreen from '../../features/adapters/AdapterSessionScreen';
import AddComputerScreen from '../../features/native/AddComputerScreen';
import EnableSharingScreen from '../../features/native/EnableSharingScreen';
import NativeSheetScreen from '../../features/native/NativeSheet';
import TerminalScreen from '../../screens/Terminal';
import RootConfirmScreen from '../../screens/RootConfirm';
import HostKeyPromptScreen from '../../screens/HostKeyPrompt';
import SshKeysScreen from '../../screens/SshKeys';
import RecordingsScreen from '../../screens/Recordings';
import RecordingPlayerScreen from '../../screens/RecordingPlayer';
import AppLockScreen from '../../screens/AppLock';
import DiagnosticsScreen from '../../screens/Diagnostics';
import NetworksScreen from '../../screens/Networks';
import AddByAddressScreen from '../../screens/AddByAddress';
import AddServerScreen from '../../screens/AddServer';
import SelfTest from '../../screens/dev/SelfTest';
import { getInitialRouteName } from '../startup';
import MainTabs from './MainTabs';
import { useIsTablet } from './orientation';
import type { RootStackParamList } from './routes';

const Stack = createNativeStackNavigator<RootStackParamList>();

// Live sessions hide the home indicator and the Android navigation bar, so a
// swipe up from the bottom edge first only reveals them. On iOS the same flag
// also defers the top edge (ScreenEdgeDeferral in RsRootViewController.swift).
const immersive = {
  autoHideHomeIndicator: true,
  navigationBarHidden: true,
} as const;

export default function RootNavigator() {
  const tablet = useIsTablet();
  return (
    <Stack.Navigator
      initialRouteName={getInitialRouteName()}
      screenOptions={{
        headerShown: false,
        contentStyle: { backgroundColor: tokens.bg },
        orientation: tablet ? 'default' : 'portrait',
      }}
    >
      <Stack.Screen
        name="OnboardTailnet"
        component={OnboardTailnetScreen}
        options={{ gestureEnabled: false }}
      />
      <Stack.Screen name="OnboardDiscover" component={OnboardDiscoverScreen} />
      <Stack.Screen name="OnboardProtect" component={OnboardProtectScreen} />
      <Stack.Screen name="MainTabs" component={MainTabs} />
      <Stack.Group
        screenOptions={{ presentation: 'transparentModal', animation: 'fade' }}
      >
        <Stack.Screen name="DeviceSheet" component={DeviceSheetScreen} />
        <Stack.Screen name="NativeSheet" component={NativeSheetScreen} />
        <Stack.Screen name="RootConfirm" component={RootConfirmScreen} />
        <Stack.Screen name="HostKeyPrompt" component={HostKeyPromptScreen} />
      </Stack.Group>
      <Stack.Group screenOptions={{ presentation: 'modal' }}>
        <Stack.Screen name="PairAdd" component={PairAddScreen} />
        <Stack.Screen
          name="PairWaiting"
          component={PairWaitingScreen}
          options={{ gestureEnabled: false }}
        />
        <Stack.Screen
          name="PairDone"
          component={PairDoneScreen}
          options={{ gestureEnabled: false }}
        />
        <Stack.Screen name="PairFile" component={PairFileScreen} />
      </Stack.Group>
      <Stack.Group
        screenOptions={{
          presentation: 'fullScreenModal',
          orientation: tablet ? 'default' : 'landscape',
          statusBarHidden: true,
          gestureEnabled: false,
          animation: 'fade',
        }}
      >
        <Stack.Screen
          name="Session"
          component={SessionScreen}
          options={immersive}
        />
        <Stack.Screen name="SessionEnded" component={SessionEndedScreen} />
        <Stack.Screen name="RecordingPlayer" component={RecordingPlayerScreen} />
        <Stack.Screen
          name="AdapterSession"
          component={AdapterSessionScreen}
          options={immersive}
        />
      </Stack.Group>
      <Stack.Screen name="DeviceRoutes" component={DeviceRoutesScreen} />
      <Stack.Screen
        name="Terminal"
        component={TerminalScreen}
        options={{ orientation: 'default' }}
      />
      {/* Settings children live in the root stack to hide the tab bar. */}
      <Stack.Screen name="SshKeys" component={SshKeysScreen} />
      <Stack.Screen name="Recordings" component={RecordingsScreen} />
      <Stack.Screen name="AppLock" component={AppLockScreen} />
      <Stack.Screen name="Diagnostics" component={DiagnosticsScreen} />
      <Stack.Screen name="Networks" component={NetworksScreen} />
      <Stack.Screen name="AddByAddress" component={AddByAddressScreen} />
      <Stack.Screen name="AddComputer" component={AddComputerScreen} />
      <Stack.Screen name="EnableSharing" component={EnableSharingScreen} />
      <Stack.Screen name="AddServer" component={AddServerScreen} />
      <Stack.Screen name="DevSelfTest" component={SelfTest} />
    </Stack.Navigator>
  );
}
