// 无界面的原生 Bridge 都放这里，AppRoot 只挂这一个。
import NetInfoBridge from '../native/netinfo/NetInfoBridge';
import LifecycleBridge from '../native/lifecycle/LifecycleBridge';
import AutoConnect from '../features/autoconnect/AutoConnect';
import PowerSync from '../features/session/PowerSync';
import TailnetSync from '../features/tailnet/TailnetSync';
import AndroidSessionBridge from '../native/androidSession/AndroidSessionBridge';
import CrashReporter from '../features/diagnostics/CrashReporter';

export default function NativeBridges(): React.JSX.Element {
  return (
    <>
      <NetInfoBridge />
      <LifecycleBridge />
      <PowerSync />
      <TailnetSync />
      <AutoConnect />
      <AndroidSessionBridge />
      <CrashReporter />
    </>
  );
}
