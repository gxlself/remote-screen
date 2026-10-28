import { Button } from '../../components/Button';
import Dialog from '../../components/Dialog';
import { common } from '../../i18n/zh/common';
import { sshKeys } from '../../i18n/zh/sshKeys';

type InstallDialogProps = {
  visible: boolean;
  serverName: string;
  installing: boolean;
  onCancel: () => void;
  onInstall: () => void;
};

export function InstallDialog({
  visible,
  serverName,
  installing,
  onCancel,
  onInstall,
}: InstallDialogProps) {
  return (
    <Dialog
      actions={[
        { label: common.cancel, onPress: onCancel, variant: 'secondary' },
      ]}
      message={sshKeys.installDialog.message(serverName)}
      onDismiss={installing ? undefined : onCancel}
      testID="install-dialog"
      title={sshKeys.installDialog.title(serverName)}
      visible={visible}
    >
      <Button
        fullWidth
        loading={installing}
        onPress={onInstall}
        title={sshKeys.install.action}
      />
    </Dialog>
  );
}

export default InstallDialog;
