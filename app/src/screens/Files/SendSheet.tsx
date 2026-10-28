import { Modal } from 'react-native';
import { BottomSheet } from '../../components/BottomSheet';
import { EmptyState } from '../../components/EmptyState';
import { Icon } from '../../components/icons/Icon';
import { ListGroup, ListRow } from '../../components/List';
import type { SendTarget } from '../../features/files/browseView';
import type { PickedFile } from '../../features/files/filesApi';
import { files } from '../../i18n/zh/files';

export function SendSheet({
  visible,
  picked,
  targets,
  busy,
  onPick,
  onClose,
}: {
  visible: boolean;
  picked: PickedFile[];
  targets: SendTarget[];
  busy: boolean;
  onPick(t: SendTarget): void;
  onClose(): void;
}) {
  const title =
    picked.length === 1
      ? files.send.one(picked[0]?.name ?? '')
      : files.send.many(picked.length);

  return (
    <Modal
      animationType="fade"
      onRequestClose={onClose}
      transparent
      visible={visible}
    >
      <BottomSheet onClose={onClose} scroll title={title}>
        {targets.length === 0 ? (
          <EmptyState icon="computer" title={files.send.emptyTitle} />
        ) : (
          <ListGroup>
            {targets.map(target => (
              <ListRow
                accessibilityLabel={files.a11y.target(
                  target.title,
                  target.subtitle,
                )}
                chevron
                disabled={busy}
                key={target.key}
                left={
                  <Icon
                    color="text2"
                    name={target.kind === 'computer' ? 'computer' : 'terminal'}
                    size={20}
                  />
                }
                onPress={() => {
                  if (!busy) {
                    onPick(target);
                  }
                }}
                subtitle={target.subtitle}
                title={target.title}
              />
            ))}
          </ListGroup>
        )}
      </BottomSheet>
    </Modal>
  );
}

export default SendSheet;
