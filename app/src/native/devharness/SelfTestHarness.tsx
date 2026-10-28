import SelfTest from '../../screens/dev/SelfTest';
import type { HarnessProps } from './registry';

export default function SelfTestHarness({ arg }: HarnessProps) {
  return <SelfTest autoConnect={arg} />;
}
