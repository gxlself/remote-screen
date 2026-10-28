import { Button } from '../../components/Button';
import { Icon } from '../../components/icons/Icon';
import { ListRow } from '../../components/List';
import { StatusChip } from '../../components/StatusChip';
import { useHostPath } from '../../features/events/EventsProvider';
import { entryName } from '../../features/native/entries';
import {
  computerSubtitle,
  nativeSubtitle,
  serverSubtitle,
} from '../../features/onboarding/discover';
import type { NativeEntry } from '../../features/onboarding/discover';
import { common } from '../../i18n/zh/common';
import { native } from '../../i18n/zh/native';
import { onboardDiscover } from '../../i18n/zh/onboardDiscover';
import type { Computer, Server } from '../../native/rscore/types';

type NativeRowProps = {
  entry: NativeEntry;
  /** Opens the same sheet as the Home card: connect, rename, set up. */
  onOpen: () => void;
};

/** V10: nothing to pair or install; the system protocol connects it. */
function NativeRow({ entry, onOpen }: NativeRowProps) {
  return (
    <ListRow
      chevron
      left={<Icon color="text2" name="computer" size={22} />}
      onPress={onOpen}
      right={<StatusChip label={native.badge.native} tone="accent" />}
      subtitle={nativeSubtitle(entry)}
      testID={`discover-native-${entry.key}`}
      title={entryName(entry)}
    />
  );
}

type RemoteRowProps = {
  computer: Computer;
  onPair: () => void;
};

function RemoteRow({ computer, onPair }: RemoteRowProps) {
  const hostPath = useHostPath(computer.hostId);
  const subtitle = computerSubtitle(computer, hostPath);

  return (
    <ListRow
      left={<Icon color="text2" name="computer" size={22} />}
      right={
        computer.paired ? (
          <StatusChip label={onboardDiscover.computer.paired} tone="good" />
        ) : (
          <Button
            accessibilityLabel={onboardDiscover.computer.pairA11y(
              computer.name,
            )}
            onPress={onPair}
            title={onboardDiscover.computer.pair}
            variant="pill"
          />
        )
      }
      subtitle={subtitle}
      testID={`discover-remote-${computer.hostId}`}
      title={computer.name}
    />
  );
}

type SshRowProps = {
  server: Server;
  onConfigure: () => void;
};

function SshRow({ server, onConfigure }: SshRowProps) {
  return (
    <ListRow
      left={<Icon color="text2" name="terminal" size={22} />}
      right={
        server.tailscaleSsh === true ? (
          <StatusChip label={onboardDiscover.ssh.direct} tone="good" />
        ) : (
          <Button
            accessibilityLabel={onboardDiscover.ssh.configureA11y(server.node)}
            onPress={onConfigure}
            title={common.settings}
            variant="pill"
          />
        )
      }
      subtitle={serverSubtitle(server)}
      testID={`discover-ssh-${server.node}`}
      title={server.node}
    />
  );
}

export type DiscoverRowProps =
  | ({ kind: 'native' } & NativeRowProps)
  | ({ kind: 'remote' } & RemoteRowProps)
  | ({ kind: 'ssh' } & SshRowProps);

export function DiscoverRow(props: DiscoverRowProps) {
  switch (props.kind) {
    case 'native':
      return <NativeRow entry={props.entry} onOpen={props.onOpen} />;
    case 'remote':
      return <RemoteRow computer={props.computer} onPair={props.onPair} />;
    case 'ssh':
      return <SshRow server={props.server} onConfigure={props.onConfigure} />;
  }
}

export default DiscoverRow;
