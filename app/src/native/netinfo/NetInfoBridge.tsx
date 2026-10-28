import { useEffect, useMemo, useRef } from 'react';
import { useCore } from '../rscore/CoreContext';
import { RsCoreError } from '../rscore/RsCoreApi';
import { useSetting } from '../../features/settings/store';
import {
  getNativeNetwork,
  setVpnPools,
  subscribeNativeNetwork,
} from './netInfo';
import { useNativeNetwork } from './nativeNetworkStore';

export default function NetInfoBridge(): null {
  const core = useCore();
  const [networks] = useSetting('networks');
  const setNative = useNativeNetwork(state => state.setNative);
  const setCore = useNativeNetwork(state => state.setCore);
  const unsupported = useRef(false);
  const workProfileUnsupported = useRef(false);
  const workProfileIfaces = useRef<string[]>([]);
  const pools = useMemo(
    () => Array.from(new Set(networks.flatMap(network => network.vpnPools))),
    [networks],
  );

  useEffect(() => {
    setVpnPools(pools);
  }, [pools]);

  useEffect(() => {
    let mounted = true;
    const updateCore = async () => {
      if (!mounted || unsupported.current) {
        return;
      }
      try {
        setCore(await core.network.status());
      } catch (error) {
        if (error instanceof RsCoreError && error.kind === 'unsupported') {
          unsupported.current = true;
          if (mounted) {
            setCore(null);
          }
        }
      }
    };

    const updateWorkProfileIfaces = (status: {
      ifaces: Array<{ name: string; workProfile?: boolean }>;
    }) => {
      if (workProfileUnsupported.current) {
        return;
      }
      const names = Array.from(
        new Set(
          status.ifaces
            .filter(iface => iface.workProfile === true)
            .map(iface => iface.name),
        ),
      ).sort();
      const previous = workProfileIfaces.current;
      if (
        names.length === previous.length &&
        names.every((name, index) => name === previous[index])
      ) {
        return;
      }
      try {
        core.settings.set('device.workProfileIfaces', names);
        workProfileIfaces.current = names;
      } catch {
        workProfileUnsupported.current = true;
        if (mounted) {
          console.warn('device.workProfileIfaces sync is unsupported');
        }
      }
    };

    const unsubscribe = subscribeNativeNetwork(status => {
      if (!mounted) {
        return;
      }
      setNative(status);
      updateWorkProfileIfaces(status);
      updateCore();
    });
    getNativeNetwork()
      .then(status => {
        if (mounted && status !== null) {
          setNative(status);
          updateWorkProfileIfaces(status);
        } else if (mounted) {
          setNative(null);
        }
      })
      .catch(() => {
        if (mounted) {
          setNative(null);
        }
      });

    return () => {
      mounted = false;
      unsubscribe();
    };
  }, [core, setCore, setNative]);

  return null;
}
