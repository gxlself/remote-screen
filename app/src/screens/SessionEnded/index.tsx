import { useEffect, useMemo } from 'react';
import { StatusBar, StyleSheet, View } from 'react-native';
import type { RootScreenProps } from '../../app/navigation/routes';
import Toast from '../../components/Toast';
import { classifyEnd } from '../../features/errors/classify';
import { FallbackGuide } from '../../features/fallback/FallbackGuide';
import { useDirectory } from '../../features/data/queries';
import { sessionEnded } from '../../i18n/zh/sessionEnded';
import { useCore } from '../../native/rscore/CoreContext';
import { boardFor } from './boardFor';
import { useEndActions } from './useEndActions';

export default function SessionEndedScreen({
  route,
  navigation,
}: RootScreenProps<'SessionEnded'>) {
  const core = useCore();
  const { directory } = useDirectory();
  const {
    hostId,
    hostName: routeHostName,
    code,
    reason,
    detail,
  } = route.params;
  const host = directory?.computers.find(
    computer => computer.hostId === hostId,
  );
  // V60: never show the raw hostId.
  const hostName = routeHostName ?? host?.name ?? sessionEnded.unknownHost;
  const classification = useMemo(
    () => classifyEnd({ code, reason }),
    [code, reason],
  );
  const endActions = useEndActions({
    route,
    navigation,
    host,
  });
  const { toastState, hideToast, guide, closeGuide, ...actions } = endActions;
  const Board = boardFor(classification.board);

  useEffect(() => {
    core.logs.write(
      'warn',
      'session-ended',
      JSON.stringify({ hostId, code, reason, detail }),
    );
  }, [code, core, detail, hostId, reason]);

  return (
    <View style={styles.root}>
      <StatusBar hidden />
      <Board
        actions={actions}
        code={classification.code}
        host={host}
        hostName={hostName}
        os={host?.os ?? null}
        params={route.params}
      />
      <FallbackGuide guide={guide} onClose={closeGuide} />
      <Toast onHide={hideToast} toast={toastState} />
    </View>
  );
}

const styles = StyleSheet.create({
  root: { flex: 1 },
});
