import { useEffect, useState } from 'react';
import { StyleSheet, View } from 'react-native';
import { common } from '../../../i18n/zh/common';
import { errBoards1 } from '../../../i18n/zh/errBoards1';
import { tokens } from '../../../theme/tokens';
import type { BoardProps } from '../boardTypes';
import { ErrorFrame } from '../ErrorFrame';
import {
  CodeBlock,
  MockWindow,
  NoteBox,
  StatusRow,
  summaryText,
} from '../parts';

const COPIED_MS = 2000;

function snippetFor(user: string): { block: string; line: string } {
  const rule = `{ "action": "accept", "src": ["${user}"], "dst": ["tag:rs-host:7400"] }`;
  return {
    block: `"acls": [\n  …\n  ${rule},\n  …\n]`,
    line: rule,
  };
}

export default function PolicyBoard({
  params,
  code,
  hostName,
  os,
  actions,
}: BoardProps) {
  const copy = errBoards1.policy;
  // The app has no account name to fill in; the snippet keeps a placeholder.
  const user = copy.fallbackUser;
  const allowedUsers = params.detail?.reason === 'allowed_users';
  const mac = os === 'macos';
  const [copied, setCopied] = useState(false);
  const snippet = snippetFor(user);

  useEffect(() => {
    if (!copied) {
      return;
    }
    const timer = setTimeout(() => setCopied(false), COPIED_MS);
    return () => clearTimeout(timer);
  }, [copied]);

  const title = allowedUsers
    ? copy.allowedUsers.title(hostName)
    : copy.title(hostName);
  const desc = allowedUsers ? copy.allowedUsers.desc(mac) : copy.desc;

  return (
    <ErrorFrame
      back={{ label: common.back, onPress: actions.back }}
      code={code}
      desc={desc}
      illustrationSummary={summaryText([], [allowedUsers ? copy.allowedUsers.note : copy.effective])}
      extra={
        allowedUsers ? undefined : (
          <View style={styles.statuses}>
            <StatusRow label={copy.targetLabel} value={copy.targetValue} />
            <StatusRow label={copy.portLabel} value={copy.portValue} />
          </View>
        )
      }
      illustration={
        <MockWindow dots={false} title={copy.windowTitle}>
          {allowedUsers ? (
            <NoteBox text={copy.allowedUsers.note} />
          ) : (
            <>
              <CodeBlock text={snippet.block} />
              <NoteBox text={copy.effective} />
            </>
          )}
        </MockWindow>
      }
      kicker={copy.kicker}
      meta={allowedUsers ? undefined : copy.meta}
      onCopyCode={actions.copyCode}
      primary={
        allowedUsers
          ? undefined
          : {
              label: copied ? common.copied : copy.copySnippet,
              onPress: () => {
                actions.copyText(snippet.line, copy.copiedToast);
                setCopied(true);
              },
            }
      }
      secondary={{ label: common.retry, onPress: () => actions.retry() }}
      title={title}
    />
  );
}

const styles = StyleSheet.create({
  statuses: {
    gap: 2,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: tokens.cardLine,
    paddingHorizontal: 10,
    paddingVertical: 4,
  },
});
