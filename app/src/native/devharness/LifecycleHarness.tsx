import { useEffect, useState } from 'react';
import { Button, ScrollView, Switch, Text, View } from 'react-native';
import type { HarnessProps } from './registry';
import type {
  BackgroundTaskEvent,
  PowerState,
} from '../specs/NativeRsLifecycle';
import {
  getPowerState,
  previousRunEndedInBackground,
  setBackgroundKeepAlive,
  setIdleTimerDisabled,
  subscribeBackgroundTask,
  subscribePowerChange,
} from '../lifecycle/lifecycle';

export default function LifecycleHarness({ log }: HarnessProps) {
  const [previous, setPrevious] = useState(false);
  const [power, setPower] = useState<PowerState | null>(null);
  const [idleDisabled, setIdleDisabled] = useState(false);
  const [keepAlive, setKeepAlive] = useState(false);

  useEffect(() => {
    const value = previousRunEndedInBackground();
    setPrevious(value);
    log('LIFECYCLE-CONST', { previousRunEndedInBackground: value });

    let mounted = true;
    getPowerState().then(next => {
      if (mounted) {
        setPower(next);
        log('LIFECYCLE-POWER', next);
      }
    });
    const unsubscribePower = subscribePowerChange((next: PowerState) => {
      if (mounted) {
        setPower(next);
        log('LIFECYCLE-POWER', next);
      }
    });
    const unsubscribeBackground = subscribeBackgroundTask((event: BackgroundTaskEvent) => {
      if (mounted) {
        log('LIFECYCLE-BG', event);
      }
    });
    return () => {
      mounted = false;
      unsubscribePower();
      unsubscribeBackground();
      setIdleTimerDisabled(false);
      setBackgroundKeepAlive(false);
    };
  }, [log]);

  const toggleIdle = (value: boolean) => {
    setIdleDisabled(value);
    setIdleTimerDisabled(value);
  };

  const toggleKeepAlive = (value: boolean) => {
    setKeepAlive(value);
    setBackgroundKeepAlive(value);
  };

  return (
    <ScrollView>
      <Text>previousRunEndedInBackground: {String(previous)}</Text>
      <Text>power: {power === null ? '—' : JSON.stringify(power)}</Text>
      <View>
        <Text>屏幕常亮</Text>
        <Switch
          accessibilityLabel="屏幕常亮"
          value={idleDisabled}
          onValueChange={toggleIdle}
        />
      </View>
      <View>
        <Text>后台保活</Text>
        <Switch
          accessibilityLabel="后台保活"
          value={keepAlive}
          onValueChange={toggleKeepAlive}
        />
      </View>
      <Button
        title="刷新电量"
        onPress={() => {
          getPowerState().then(next => {
            setPower(next);
            log('LIFECYCLE-POWER', next);
          });
        }}
      />
    </ScrollView>
  );
}
