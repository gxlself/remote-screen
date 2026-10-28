import { createBottomTabNavigator } from '@react-navigation/bottom-tabs';
import TabBar from '../../components/TabBar';
import { useLiveSessionCount } from '../../features/session/useLiveSessionCount';
import { zh } from '../../i18n/zh';
import HomeScreen from '../../screens/Home';
import SessionsScreen from '../../screens/Sessions';
import FilesScreen from '../../screens/Files';
import SettingsScreen from '../../screens/Settings';
import type { MainTabParamList } from './routes';

const Tab = createBottomTabNavigator<MainTabParamList>();
const renderTabBar: NonNullable<
  React.ComponentProps<typeof Tab.Navigator>['tabBar']
> = props => <TabBar {...props} />;

export default function MainTabs() {
  const n = useLiveSessionCount();
  return (
    <Tab.Navigator screenOptions={{ headerShown: false }} tabBar={renderTabBar}>
      <Tab.Screen
        name="Home"
        component={HomeScreen}
        options={{ title: zh.common.tabs.devices }}
      />
      <Tab.Screen
        name="Sessions"
        component={SessionsScreen}
        options={{
          title: zh.common.tabs.sessions,
          tabBarBadge: n > 0 ? n : undefined,
          tabBarAccessibilityLabel:
            n > 0
              ? `${zh.common.tabs.sessions}，${zh.common.a11y.sessionsBadge(n)}`
              : undefined,
        }}
      />
      <Tab.Screen
        name="Files"
        component={FilesScreen}
        options={{ title: zh.common.tabs.files }}
      />
      <Tab.Screen
        name="Settings"
        component={SettingsScreen}
        options={{ title: zh.common.tabs.settings }}
      />
    </Tab.Navigator>
  );
}
