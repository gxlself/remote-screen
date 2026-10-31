import type { JSX } from 'react';
import { Modal } from 'react-native';
import { BottomSheet } from '../../components/BottomSheet';
import { ListGroup, ListRow } from '../../components/List';
import { Switch } from '../../components/Switch';
import { adapters } from '../../i18n/zh/adapters';
import type { AdapterProtocol } from '../../native/rscore/types';

export type AdapterMoreSheetProps = {
  protocol: AdapterProtocol;
  /** RDP 的声音开关（设置 session.sound）。 */
  sound: boolean;
  onSound(on: boolean): void;
  /** 收起工具栏时不留把手（toolbarDock.ts，按设备记住）。 */
  handleHidden: boolean;
  onHandleHidden(hidden: boolean): void;
  /** 自动同步关着、电脑上复制过东西时才有。 */
  hasRemoteClipboard: boolean;
  /** 这台电脑存了凭据。 */
  canForget: boolean;
  onCtrlAltDel(): void;
  onCopyRemoteClipboard(): void;
  onGestures(): void;
  /** Only while the picture is zoomed in. */
  onFitScreen?(): void;
  onForget(): void;
  onClose(): void;
};

/** Less frequent session actions, kept off the toolbar. */
export function AdapterMoreSheet(p: AdapterMoreSheetProps): JSX.Element {
  return (
    <Modal animationType="fade" onRequestClose={p.onClose} transparent visible>
      <BottomSheet
        onClose={p.onClose}
        testID="adapter-more"
        title={adapters.more.title}
      >
        <ListGroup>
          {p.protocol === 'rdp' ? (
            <ListRow
              onPress={p.onCtrlAltDel}
              subtitle={adapters.more.ctrlAltDelHint}
              testID="adapter-more-ctrlAltDel"
              title={adapters.more.ctrlAltDel}
            />
          ) : null}
          {p.protocol === 'rdp' ? (
            <ListRow
              right={
                <Switch
                  accessibilityLabel={adapters.more.sound}
                  onValueChange={p.onSound}
                  testID="adapter-more-sound"
                  value={p.sound}
                />
              }
              title={adapters.more.sound}
            />
          ) : null}
          {p.hasRemoteClipboard ? (
            <ListRow
              onPress={p.onCopyRemoteClipboard}
              subtitle={adapters.more.remoteClipboardHint}
              testID="adapter-more-remoteClipboard"
              title={adapters.more.remoteClipboard}
            />
          ) : null}
          {p.onFitScreen === undefined ? null : (
            <ListRow
              onPress={p.onFitScreen}
              subtitle={adapters.more.fitScreenHint}
              testID="adapter-more-fitScreen"
              title={adapters.more.fitScreen}
            />
          )}
          <ListRow
            right={
              <Switch
                accessibilityLabel={adapters.more.hideHandle}
                onValueChange={p.onHandleHidden}
                testID="adapter-more-hideHandle"
                value={p.handleHidden}
              />
            }
            subtitle={adapters.more.hideHandleHint}
            title={adapters.more.hideHandle}
          />
          <ListRow
            onPress={p.onGestures}
            testID="adapter-more-gestures"
            title={adapters.more.gestures}
          />
          {p.canForget ? (
            <ListRow
              onPress={p.onForget}
              testID="adapter-more-forget"
              title={adapters.more.forget}
              tone="danger"
            />
          ) : null}
        </ListGroup>
      </BottomSheet>
    </Modal>
  );
}

export default AdapterMoreSheet;
