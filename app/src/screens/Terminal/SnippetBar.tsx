import { Pressable, ScrollView, StyleSheet } from 'react-native';
import AppText from '../../components/AppText';
import { IconButton } from '../../components/IconButton';
import { snippetLabel } from '../../features/terminal/snippets';
import type { Snippet } from '../../features/terminal/snippets';
import { terminal } from '../../i18n/zh/terminal';
import { isAndroid, press, ripple } from '../../theme/platform';
import { hit, radius, space, tokens } from '../../theme/tokens';

// A long command wraps inside a narrow pill instead of stretching the bar;
// the editor shows it in full.
const PILL_MAX_WIDTH = 200;

export function SnippetBar(p: {
  items: Snippet[];
  onSend: (snippet: Snippet) => void;
  onEdit: () => void;
  showEdit?: boolean;
  // Not connected: the pills dim but still say why when pressed.
  connected?: boolean;
}) {
  const snippetHitSlop = (hit.min - 36) / 2;
  const android = isAndroid();
  const dimmed = p.connected === false;
  const pillStyle = ({ pressed }: { pressed: boolean }) => [
    styles.pill,
    pressed && !android ? styles.pressed : null,
  ];
  return (
    <ScrollView
      accessibilityLabel={terminal.snippets}
      contentContainerStyle={styles.content}
      horizontal
      showsHorizontalScrollIndicator={false}
      style={styles.bar}
    >
      {p.items.length === 0 ? (
        <Pressable
          accessibilityLabel={terminal.addSnippet}
          accessibilityRole="button"
          android_ripple={android ? { color: ripple.neutral } : undefined}
          hitSlop={snippetHitSlop}
          onPress={p.onEdit}
          style={pillStyle}
        >
          <AppText variant="mono">{terminal.addSnippet}</AppText>
        </Pressable>
      ) : (
        <>
          {p.items.map(snippet => (
            <Pressable
              accessibilityLabel={
                snippet.run
                  ? terminal.sendCommand(snippetLabel(snippet))
                  : terminal.inputCommand(snippetLabel(snippet))
              }
              accessibilityRole="button"
              android_ripple={android ? { color: ripple.neutral } : undefined}
              hitSlop={snippetHitSlop}
              key={snippet.id}
              onPress={() => p.onSend(snippet)}
              style={pillStyle}
              testID={`snippet-pill-${snippet.id}`}
            >
              <AppText color={dimmed ? 'text3' : 'text'} variant="mono">
                {snippetLabel(snippet)}
              </AppText>
            </Pressable>
          ))}
          {p.showEdit !== false ? (
            <IconButton
              accessibilityLabel={terminal.a11y.editSnippets}
              icon="more"
              onPress={p.onEdit}
              size={36}
            />
          ) : null}
        </>
      )}
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  bar: { minHeight: 48, flexGrow: 0, flexShrink: 0 },
  content: { paddingHorizontal: space.md, gap: space.sm, alignItems: 'center' },
  pill: {
    minHeight: 36,
    maxWidth: PILL_MAX_WIDTH,
    borderRadius: radius.thumb,
    backgroundColor: tokens.raised,
    paddingHorizontal: space.md,
    justifyContent: 'center',
    overflow: 'hidden',
  },
  pressed: { opacity: press.opacity },
});
