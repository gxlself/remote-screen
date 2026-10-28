import { useState } from 'react';
import type { JSX } from 'react';
import { Modal } from 'react-native';
import type { RootStackParamList } from '../../app/navigation/routes';
import { BottomSheet } from '../../components/BottomSheet';
import { useDirectory } from '../data/queries';
import { adapters } from '../../i18n/zh/adapters';
import { useCore } from '../../native/rscore/CoreContext';
import type { AdapterTarget } from '../../native/rscore/types';
import {
  adaptersAvailable,
  adapterProtocolFor,
  BUILTIN_PROTOCOLS,
  targetForComputer,
} from './logic';
import { ListGroup, ListRow } from '../../components/List';
import { MonitorCountPicker } from './MonitorCountPicker';
import { useMonitorCount } from './monitorMemory';

export type AdapterEntryRowProps = {
  hostId: string;
  onOpen: (params: RootStackParamList['AdapterSession']) => void;
};

/** the same remembered count the session toolbar changes. */
function MonitorCountRow({ target }: { target: AdapterTarget }): JSX.Element {
  const [count, setCount] = useMonitorCount(target);
  const [open, setOpen] = useState(false);
  return (
    <>
      <ListRow
        chevron
        onPress={() => setOpen(true)}
        testID="adapter-monitors-row"
        title={adapters.monitors.title}
        value={adapters.monitors.value(count)}
      />
      {open ? (
        <Modal
          animationType="fade"
          onRequestClose={() => setOpen(false)}
          transparent
          visible
        >
          <BottomSheet
            onClose={() => setOpen(false)}
            testID="adapter-monitors-sheet"
            title={adapters.monitors.title}
          >
            <MonitorCountPicker
              count={count}
              onChange={next => {
                setCount(next);
                setOpen(false);
              }}
            />
          </BottomSheet>
        </Modal>
      ) : null}
    </>
  );
}

export function AdapterEntryRow({
  hostId,
  onOpen,
}: AdapterEntryRowProps): JSX.Element | null {
  const core = useCore();
  const { directory } = useDirectory();

  if (!adaptersAvailable(core)) {
    return null;
  }

  const computer = directory?.computers.find(
    candidate => candidate.hostId === hostId,
  );
  if (computer === undefined || !computer.online) {
    return null;
  }

  const protocol = adapterProtocolFor(computer.os);
  if (!(BUILTIN_PROTOCOLS as readonly string[]).includes(protocol)) {
    return null;
  }
  const entry = targetForComputer(computer);
  if (entry === null) {
    return null;
  }

  const context = computer.console?.loginWindow
    ? 'loginWindow'
    : !computer.hostRunning
    ? 'noHost'
    : 'manual';

  return (
    <ListGroup>
      <ListRow
        chevron
        onPress={() =>
          onOpen({
            target: entry.target,
            name: computer.name,
            addrKind: entry.addrKind,
            context,
          })
        }
        subtitle={adapters.entry.hint(protocol)}
        testID="adapter-entry"
        title={adapters.entry[protocol]}
      />
      {protocol === 'rdp' ? <MonitorCountRow target={entry.target} /> : null}
    </ListGroup>
  );
}
