import { common } from '../../../i18n/zh/common';
import { errBoards2 } from '../../../i18n/zh/errBoards2';
import { native } from '../../../i18n/zh/native';
import type { BoardProps } from '../boardTypes';
import { ErrorFrame } from '../ErrorFrame';

export default function NoHostBoard({
  code,
  hostName,
  os,
  actions,
}: BoardProps) {
  const copy = errBoards2.noHost;
  // Without a directory entry the system is unknown; name both tools.
  const knownOs = os ?? undefined;
  const desc = `${copy.desc}${native.noHost.desc(knownOs)}`;

  return (
    <ErrorFrame
      back={{ label: common.back, onPress: actions.back }}
      code={code}
      desc={desc}
      kicker={copy.kicker}
      onCopyCode={actions.copyCode}
      primary={{
        label: native.noHost.connect(knownOs),
        onPress: () => {
          Promise.resolve(actions.openScreenSharing()).catch(() => undefined);
        },
      }}
      secondary={{ label: copy.retry, onPress: () => actions.retry() }}
      title={copy.title(hostName)}
    />
  );
}
