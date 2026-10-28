import { useMemo, useRef, useState } from 'react';
import {
  Pressable,
  ScrollView,
  StyleSheet,
  View,
} from 'react-native';
import { useWindowSize } from '../../features/session/WindowContext';
import AppText from '../../components/AppText';
import { Button } from '../../components/Button';
import { Dialog } from '../../components/Dialog';
import { SectionHeader } from '../../components/List';
import SessionPanel from '../../components/SessionPanel';
import { Switch } from '../../components/Switch';
import { Icon } from '../../components/icons/Icon';
import { clipboardView } from '../../features/session/clipboardView';
import type { ClipboardViewItem } from '../../features/session/clipboardView';
import { useSetting } from '../../features/settings/store';
import { clipboardMore } from '../../i18n/zh/clipboardMore';
import { usePlatform } from '../../native/platform/PlatformContext';
import { useCore } from '../../native/rscore/CoreContext';
import { hit, radius, tokens } from '../../theme/tokens';
import type { SheetProps } from './slots';

/** 超过这么多字的「粘贴为键入」先确认。 */
const LONG_TYPING_CHARS = 200;

export function ClipboardSheet({ ctx, onClose }: SheetProps) {
  const { height: windowHeight } = useWindowSize();
  const core = useCore();
  const platform = usePlatform();
  const [autoSync, setAutoSync] = useSetting('clipboard.autoSync');
  const items = useMemo(() => ctx.rec?.clipboard ?? [], [ctx.rec]);
  const view = useMemo(() => clipboardView(items, autoSync), [autoSync, items]);
  const copy = clipboardMore.clipboard;
  const sessionId = ctx.sessionId;
  const disabled = ctx.view.inputBlocked || sessionId === null;

  const [longTyping, setLongTyping] = useState<string | null>(null);
  const busy = useRef(false);

  /** 发到电脑；失败时提示并返回 false。 */
  const sendText = (text: string, typing: boolean): boolean => {
    if (sessionId === null) {
      return false;
    }
    try {
      if (typing) {
        core.sessions.text(sessionId, text);
      } else {
        core.sessions.clipboardSend(sessionId, text);
      }
    } catch {
      ctx.showToast(copy.sendFailed, { tone: 'bad' });
      return false;
    }
    return true;
  };

  const typeText = (text: string): void => {
    if (sendText(text, true)) {
      ctx.showToast(copy.typedToComputer(Array.from(text).length), {
        tone: 'good',
      });
    }
  };

  const paste = async (typing: boolean): Promise<void> => {
    let text: string | null;
    try {
      text = await platform.readText();
    } catch {
      ctx.showToast(copy.phoneReadFailed, { tone: 'bad' });
      return;
    }
    if (text === null || text === '') {
      ctx.showToast(copy.phoneClipboardEmpty);
      return;
    }
    if (!typing) {
      if (sendText(text, false)) {
        ctx.showToast(copy.sentToComputer);
      }
      return;
    }
    // 逐字键入很慢，长文本先确认，免得电脑上被敲进一大段。
    if (Array.from(text).length > LONG_TYPING_CHARS) {
      setLongTyping(text);
      return;
    }
    typeText(text);
  };

  const guarded = (task: () => Promise<void>): void => {
    if (busy.current) {
      return;
    }
    busy.current = true;
    task().finally(() => {
      busy.current = false;
    });
  };

  const reuse = async (item: ClipboardViewItem): Promise<void> => {
    if (item.direction === 'fromHost') {
      try {
        await platform.copyText(item.text);
      } catch {
        ctx.showToast(copy.copyFailed, { tone: 'bad' });
        return;
      }
      ctx.showToast(copy.copiedToPhone);
      return;
    }
    if (sendText(item.text, false)) {
      ctx.showToast(copy.resentToComputer);
    }
  };

  return (
    <SessionPanel
      backdropLabel={copy.backdrop}
      kind="popover"
      onClose={onClose}
      testID="clipboard-sheet"
      title={copy.title}
    >
      <View
        testID="clipboard-body"
        style={[
          styles.body,
          { maxHeight: Math.min(430, Math.max(180, windowHeight - 132)) },
        ]}
      >
        <ScrollView
          contentContainerStyle={styles.scrollContent}
          keyboardShouldPersistTaps="handled"
          style={styles.scroll}
        >
          <View style={styles.section}>
            <SectionHeader title={copy.latestTitle} />
            <View accessible={false} style={styles.preview}>
              {view.latestFromHost === null ? (
                <>
                  <AppText color="text3" variant="caption">
                    {copy.emptyTitle}
                  </AppText>
                  <AppText color="text3" variant="caption">
                    {copy.emptyHint}
                  </AppText>
                </>
              ) : (
                <>
                  <AppText
                    color="text"
                    mono
                    numberOfLines={4}
                    selectable
                    testID="clipboard-latest-text"
                    variant="monoSmall"
                  >
                    {view.latestFromHost.text}
                  </AppText>
                  <View style={styles.metaRow}>
                    <View
                      style={[
                        styles.dot,
                        {
                          backgroundColor: autoSync
                            ? tokens.good
                            : tokens.muted,
                        },
                      ]}
                    />
                    <AppText
                      color="text3"
                      testID="clipboard-meta"
                      variant="caption"
                    >
                      {view.meta}
                    </AppText>
                  </View>
                </>
              )}
            </View>
          </View>

          <View style={styles.section}>
            <SectionHeader title={copy.sendTitle} />
            <View style={styles.actions}>
              <View style={styles.action}>
                <Button
                  accessibilityLabel={copy.pasteToComputer}
                  align="start"
                  disabled={disabled}
                  icon="paste"
                  onPress={() => guarded(() => paste(false))}
                  testID="clipboard-paste"
                  title={copy.pasteToComputer}
                  variant="primary"
                />
              </View>
              <View style={styles.action}>
                <Button
                  accessibilityLabel={copy.pasteAsTyping}
                  accessibilityHint={copy.pasteAsTypingHint}
                  align="start"
                  disabled={disabled}
                  icon="keyboard"
                  onPress={() => guarded(() => paste(true))}
                  testID="clipboard-type"
                  title={copy.pasteAsTyping}
                  variant="secondary"
                />
                <AppText
                  accessibilityElementsHidden
                  color="text3"
                  importantForAccessibility="no"
                  variant="caption"
                >
                  {copy.pasteAsTypingHint}
                </AppText>
              </View>
            </View>
          </View>

          {view.recent.length > 0 ? (
            <View style={styles.section}>
              <SectionHeader title={copy.recentTitle} />
              <View
                accessibilityLabel={copy.recentA11y}
                accessibilityRole="list"
                testID="clipboard-recent"
              >
                {view.recent.map((item, index) => (
                  <Pressable
                    accessibilityLabel={item.a11y}
                    accessibilityRole="button"
                    key={`${item.direction}-${index}`}
                    onPress={() => guarded(() => reuse(item))}
                    style={({ pressed }) => [
                      styles.recentRow,
                      index === 0 ? null : styles.recentDivider,
                      pressed ? styles.pressed : null,
                    ]}
                  >
                    <AppText
                      color="text2"
                      mono
                      numberOfLines={1}
                      variant="mono"
                    >
                      {item.text}
                    </AppText>
                    <AppText color="text3" variant="caption">
                      {item.label}
                    </AppText>
                  </Pressable>
                ))}
              </View>
            </View>
          ) : null}
        </ScrollView>

        <View style={styles.syncRow}>
          <Icon color="text3" name="clipboard" size={18} />
          <AppText color="text" style={styles.syncLabel} variant="body">
            {copy.autoSync}
          </AppText>
          <Switch
            accessibilityLabel={copy.autoSync}
            onValueChange={setAutoSync}
            testID="clipboard-auto-sync"
            value={autoSync}
          />
        </View>
      </View>
      <Dialog
        actions={[
          {
            label: copy.cancel,
            onPress: () => setLongTyping(null),
            variant: 'secondary',
          },
          {
            label: copy.longTypingConfirm,
            onPress: () => {
              const text = longTyping;
              setLongTyping(null);
              if (text !== null) {
                typeText(text);
              }
            },
            testID: 'clipboard-long-typing-confirm',
          },
        ]}
        message={copy.longTypingBody}
        onDismiss={() => setLongTyping(null)}
        testID="clipboard-long-typing"
        title={copy.longTypingTitle(
          longTyping === null ? 0 : Array.from(longTyping).length,
        )}
        visible={longTyping !== null}
      />
    </SessionPanel>
  );
}

const styles = StyleSheet.create({
  body: {
    gap: 16,
    paddingBottom: 10,
  },
  scroll: { flexShrink: 1 },
  scrollContent: { gap: 14 },
  section: { gap: 8 },
  preview: {
    gap: 8,
    paddingHorizontal: 12,
    paddingVertical: 10,
    borderRadius: radius.button,
    backgroundColor: tokens.raised,
  },
  metaRow: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'flex-end',
    gap: 6,
  },
  dot: {
    width: 6,
    height: 6,
    borderRadius: 3,
  },
  actions: {
    flexDirection: 'row',
    alignItems: 'flex-start',
    gap: 10,
  },
  action: { flex: 1, gap: 6 },
  recentRow: {
    minHeight: hit.min,
    justifyContent: 'center',
    paddingHorizontal: 12,
    paddingVertical: 6,
    gap: 2,
    borderRadius: radius.button,
    backgroundColor: tokens.raised,
  },
  recentDivider: { marginTop: 6 },
  pressed: { opacity: 0.8 },
  syncRow: {
    minHeight: hit.min,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    borderTopWidth: 1,
    borderTopColor: tokens.cardLine,
    paddingTop: 4,
  },
  syncLabel: { flex: 1 },
});

export default ClipboardSheet;
