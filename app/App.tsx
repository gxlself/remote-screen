import { useState } from 'react';
import AppRoot from './src/app/AppRoot';
import { parseLaunchProps, setLaunchOptions } from './src/app/launchOptions';
import DevHarness from './src/native/devharness/DevHarness';

export default function App(props: Record<string, unknown>) {
  const [{ devHarness, harnessArg }] = useState(() => {
    const options = parseLaunchProps(props);
    setLaunchOptions(options);
    return options;
  });
  return devHarness !== undefined ? (
    <DevHarness name={devHarness} arg={harnessArg} />
  ) : (
    <AppRoot />
  );
}
