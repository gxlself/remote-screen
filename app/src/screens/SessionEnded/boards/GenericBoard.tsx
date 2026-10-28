import { classifyEnd } from '../../../features/errors/classify';
import type { GenericKey } from '../../../features/errors/classify';
import { useTailnetEnabled } from '../../../features/tailnet/enabled';
import { sessionEnded } from '../../../i18n/zh/sessionEnded';
import type { BoardProps, EndAction } from '../boardTypes';
import { ErrorFrame } from '../ErrorFrame';

function genericKeyFor(params: BoardProps['params']): GenericKey {
  return classifyEnd(params).genericKey ?? 199;
}

export default function GenericBoard({
  params,
  code,
  hostName,
  actions,
}: BoardProps) {
  const key = genericKeyFor(params);
  const tailnetEnabled = useTailnetEnabled();
  const copy = sessionEnded.generic[key];
  const title = copy.title(hostName);
  // 131 suggests Tailscale only while it is switched on.
  const desc =
    key === 131 && !tailnetEnabled
      ? sessionEnded.generic[131].descLan
      : copy.desc(hostName);
  const back = { label: sessionEnded.back, onPress: actions.back };
  let primary: EndAction | undefined;
  let more: EndAction[] = [];

  switch (key) {
    case 142:
      primary = { label: sessionEnded.retry, onPress: actions.retry };
      break;
    case 180:
      primary = {
        label: sessionEnded.pairAgain,
        onPress: () => actions.pair('scan'),
      };
      break;
    case 181:
      primary = { label: sessionEnded.goPair, onPress: () => actions.pair() };
      break;
    case 199:
      primary = { label: sessionEnded.retry, onPress: actions.retry };
      more = [
        {
          label: sessionEnded.diagnostics,
          onPress: actions.openDiagnostics,
        },
      ];
      break;
    case 'idleTimeout':
      primary = { label: sessionEnded.retry, onPress: actions.retry };
      break;
  }

  return (
    <ErrorFrame
      back={back}
      code={code}
      desc={desc}
      kicker={copy.kicker}
      more={more}
      onCopyCode={actions.copyCode}
      primary={primary}
      title={title}
    />
  );
}
