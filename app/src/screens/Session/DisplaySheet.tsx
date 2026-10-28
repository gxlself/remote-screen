import { useEffect, useState } from 'react';
import {
  Dimensions,
  PixelRatio,
  Pressable,
  ScrollView,
  StyleSheet,
  View,
} from 'react-native';
import { useWindowSize } from '../../features/session/WindowContext';
import AppText from '../../components/AppText';
import { Button } from '../../components/Button';
import { Dialog } from '../../components/Dialog';
import { IconButton } from '../../components/IconButton';
import { formatResolution } from '../../features/format/latency';
import {
  canCreate,
  useVirtualDisplay,
  virtualDisplayOpts,
} from '../../features/vdisplay';
import { common } from '../../i18n/zh/common';
import { qualityDisplay } from '../../i18n/zh/qualityDisplay';
import { vdisplay } from '../../i18n/zh/vdisplay';
import type { SessionDisplay } from '../../native/rscore/types';
import { tokens } from '../../theme/tokens';
import { useCore } from '../../native/rscore/CoreContext';
import type { SheetProps } from './slots';
import { useTitleFocus } from './useTitleFocus';

const text = qualityDisplay.display;
const THUMB_WIDTH = 272;
const PORTRAIT_WIDTH = 108;
const PORTRAIT_HEIGHT = 192;
const ROW_GAP = 14;
const SHEET_PADDING = 24;
const MIN_SCALE = 0.35;
/** 移除虚拟屏后等它从显示器列表里消失的最长时间。 */
const REMOVE_TIMEOUT_MS = 10_000;

type ThumbSize = { width: number; height: number };

function thumbSize(display: SessionDisplay): ThumbSize {
  if (display.widthPx <= 0 || display.heightPx <= 0) {
    return { width: THUMB_WIDTH, height: 170 };
  }
  if (display.heightPx > display.widthPx) {
    return { width: PORTRAIT_WIDTH, height: PORTRAIT_HEIGHT };
  }
  return {
    width: THUMB_WIDTH,
    height: Math.round((THUMB_WIDTH * display.heightPx) / display.widthPx),
  };
}

function displayName(display: SessionDisplay, index: number): string {
  const base = text.name(index + 1);
  const primary = display.primary ? text.primarySuffix : '';
  const portrait =
    display.heightPx > display.widthPx ? text.portraitSuffix : '';
  return `${base}${primary}${portrait}`;
}

/** 摆得下的缩略图宽度：三块以上或分屏时按可用宽度等比缩小，不裁掉。 */
function fitScale(sizes: readonly ThumbSize[], availableWidth: number): number {
  if (sizes.length === 0) {
    return 1;
  }
  const thumbs = sizes.reduce((sum, size) => sum + size.width, 0);
  const room = availableWidth - ROW_GAP * (sizes.length - 1);
  return thumbs <= room ? 1 : Math.max(MIN_SCALE, room / thumbs);
}

export function DisplaySheet({ ctx, onClose }: SheetProps) {
  const core = useCore();
  const titleRef = useTitleFocus();
  const { width: windowWidth } = useWindowSize();
  const displays = ctx.rec?.displays ?? [];
  const currentId = ctx.rec?.displayId ?? null;
  const screen = Dimensions.get('screen');
  const screenInfo = {
    width: screen.width,
    height: screen.height,
    pixelRatio: PixelRatio.get(),
  };
  const createOpts = virtualDisplayOpts(screenInfo);
  const virtualDisplay = useVirtualDisplay({
    sessionId: ctx.sessionId,
    displays,
    onCreated: () => {
      ctx.showToast(vdisplay.created, { tone: 'good' });
      onClose();
    },
  });
  const [confirmRemove, setConfirmRemove] = useState(false);
  const [removing, setRemoving] = useState<{
    id: number;
    failed: boolean;
  } | null>(null);
  const showCreate = canCreate({
    features: ctx.host?.features,
    role: ctx.rec?.role,
    sessionId: ctx.sessionId,
    ownedId: virtualDisplay.ownedId,
  });
  const ownedId = virtualDisplay.ownedId;
  const removingId = removing?.id ?? null;
  const removed =
    removingId !== null && !displays.some(item => item.id === removingId);

  useEffect(() => {
    if (!removed) {
      return;
    }
    setRemoving(null);
    ctx.showToast(vdisplay.removed, { tone: 'good' });
    onClose();
    // 只在虚拟屏真的从列表里消失时提示一次。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [removed]);

  useEffect(() => {
    if (removingId === null) {
      return undefined;
    }
    const timer = setTimeout(
      () =>
        setRemoving(current =>
          current?.id === removingId ? { ...current, failed: true } : current,
        ),
      REMOVE_TIMEOUT_MS,
    );
    return () => clearTimeout(timer);
  }, [removingId]);

  const choose = (display: SessionDisplay) => {
    // 点一下就切过去，切好后会话页会提示「已切到显示器 N」。
    if (display.id !== currentId && ctx.sessionId !== null) {
      core.sessions.selectDisplay(ctx.sessionId, display.id);
    }
    onClose();
  };

  const remove = () => {
    setConfirmRemove(false);
    if (ownedId === null) {
      return;
    }
    try {
      virtualDisplay.release(ownedId);
    } catch {
      ctx.showToast(vdisplay.removeFailed, { tone: 'bad' });
      return;
    }
    setRemoving({ id: ownedId, failed: false });
  };

  const sizes = displays.map(thumbSize);
  const scale = fitScale(sizes, Math.max(0, windowWidth - SHEET_PADDING * 2));

  return (
    <View
      accessibilityViewIsModal
      onAccessibilityEscape={onClose}
      style={styles.root}
      testID="display-sheet"
    >
      <Pressable accessible={false} onPress={onClose} style={styles.backdrop} />
      <View style={styles.header}>
        <View style={styles.headerSpacer} />
        <View style={styles.titleBlock}>
          <View
            ref={titleRef as never}
            accessible
            accessibilityLabel={text.title}
            accessibilityRole="header"
          >
            <AppText
              center
              style={styles.title}
              variant="headline"
              weight={600}
            >
              {text.title}
            </AppText>
          </View>
          <AppText center color="text3" variant="caption">
            {ctx.os === 'windows'
              ? text.subtitleWindows(ctx.hostName)
              : text.subtitle(ctx.hostName)}
          </AppText>
        </View>
        <IconButton
          accessibilityLabel={common.a11y.close}
          icon="close"
          onPress={onClose}
          size={44}
          testID="display-close"
          variant="raised"
        />
      </View>

      <ScrollView style={styles.rowScroll}>
        <View
          accessibilityLabel={text.group}
          accessibilityRole="radiogroup"
          style={styles.row}
        >
          {displays.map((display, index) => {
            const base = sizes[index] ?? thumbSize(display);
            const size = {
              width: Math.floor(base.width * scale),
              height: Math.floor(base.height * scale),
            };
            const isCurrent = display.id === currentId;
            const name = displayName(display, index);
            const meta = text.meta(
              formatResolution(display.widthPx, display.heightPx),
              display.refreshHz ?? null,
            );
            const virtualSuffix = display.isVirtual ? ` ${vdisplay.badge}` : '';
            return (
              <Pressable
                key={display.id}
                accessibilityHint={isCurrent ? undefined : text.switchHint}
                accessibilityLabel={`${name}${
                  isCurrent ? ` ${text.viewing}` : ''
                }${virtualSuffix} ${meta}`}
                accessibilityRole="radio"
                accessibilityState={{
                  checked: isCurrent,
                  selected: isCurrent,
                }}
                onPress={() => choose(display)}
                style={({ pressed }) => [
                  styles.display,
                  pressed ? styles.displayPressed : null,
                ]}
              >
                <View
                  style={[
                    styles.thumb,
                    { width: size.width, height: size.height },
                    isCurrent ? styles.thumbSelected : styles.thumbIdle,
                  ]}
                  testID={`display-thumb-${display.id}`}
                >
                  <AppText color="text2" variant="headline" weight={600}>
                    {String(index + 1)}
                  </AppText>
                </View>
                <View style={styles.caption}>
                  <View style={styles.nameRow}>
                    {isCurrent ? <View style={styles.dot} /> : null}
                    {isCurrent ? (
                      <AppText color="good" variant="caption" weight={500}>
                        {text.viewing}
                      </AppText>
                    ) : null}
                    <AppText color="text" variant="footnote" weight={500}>
                      {name}
                    </AppText>
                    {display.isVirtual ? (
                      <AppText
                        color="text3"
                        testID={`display-virtual-${display.id}`}
                        variant="caption"
                      >
                        {vdisplay.badge}
                      </AppText>
                    ) : null}
                  </View>
                  <AppText color="text3" mono variant="monoSmall">
                    {meta}
                  </AppText>
                </View>
              </Pressable>
            );
          })}
        </View>
      </ScrollView>

      <View style={styles.footer}>
        <View style={styles.vdisplayActions}>
          {virtualDisplay.state.kind === 'pending' ? (
            <AppText
              accessibilityLiveRegion="polite"
              color="text3"
              testID="display-vd-pending"
              variant="body"
            >
              {vdisplay.creating}
            </AppText>
          ) : showCreate && removing === null ? (
            <Button
              accessibilityLabel={vdisplay.createA11y(
                createOpts.widthPx,
                createOpts.heightPx,
              )}
              onPress={() => virtualDisplay.create(screenInfo)}
              testID="display-vd-create"
              title={vdisplay.create(createOpts.widthPx, createOpts.heightPx)}
              variant="secondary"
            />
          ) : null}
          {virtualDisplay.state.kind === 'error' ? (
            <AppText color="bad" testID="display-vd-error" variant="footnote">
              {virtualDisplay.state.text}
            </AppText>
          ) : null}
          {removing !== null ? (
            <AppText
              accessibilityLiveRegion="polite"
              color={removing.failed ? 'bad' : 'text3'}
              testID="display-vd-removing"
              variant="footnote"
            >
              {removing.failed ? vdisplay.removeSlow : vdisplay.removing}
            </AppText>
          ) : ownedId !== null ? (
            <Button
              onPress={() => setConfirmRemove(true)}
              testID="display-vd-remove"
              title={vdisplay.remove}
              variant="text"
            />
          ) : null}
        </View>
      </View>
      <Dialog
        actions={[
          {
            label: common.cancel,
            onPress: () => setConfirmRemove(false),
            variant: 'secondary',
          },
          {
            label: vdisplay.removeConfirm,
            onPress: remove,
            testID: 'display-vd-remove-confirm',
            variant: 'pillDanger',
          },
        ]}
        message={vdisplay.removeConfirmBody}
        onDismiss={() => setConfirmRemove(false)}
        testID="display-vd-remove-dialog"
        title={vdisplay.removeConfirmTitle}
        visible={confirmRemove}
      />
    </View>
  );
}

const styles = StyleSheet.create({
  root: {
    position: 'absolute',
    top: 0,
    right: 0,
    bottom: 0,
    left: 0,
    alignItems: 'center',
    justifyContent: 'space-between',
    padding: SHEET_PADDING,
    backgroundColor: 'rgba(0,0,0,0.6)',
  },
  backdrop: {
    position: 'absolute',
    top: 0,
    right: 0,
    bottom: 0,
    left: 0,
  },
  header: {
    width: '100%',
    maxWidth: 680,
    flexDirection: 'row',
    alignItems: 'flex-start',
    minHeight: 44,
  },
  titleBlock: {
    flex: 1,
    alignItems: 'center',
    gap: 4,
  },
  headerSpacer: {
    width: 44,
    height: 44,
  },
  title: {
    fontSize: 17,
  },
  row: {
    width: '100%',
    flexGrow: 1,
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'center',
    gap: ROW_GAP,
  },
  rowScroll: { width: '100%', flexShrink: 1 },
  display: {
    alignItems: 'center',
    gap: 10,
  },
  displayPressed: { opacity: 0.7 },
  thumb: {
    alignItems: 'center',
    justifyContent: 'center',
    borderRadius: 8,
    backgroundColor: tokens.thumbBg,
  },
  thumbSelected: {
    borderWidth: 2.5,
    borderColor: tokens.accent,
  },
  thumbIdle: {
    borderWidth: 1,
    borderColor: 'rgba(255,255,255,0.18)',
  },
  caption: {
    alignItems: 'center',
    gap: 2,
  },
  nameRow: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
  },
  dot: {
    width: 6,
    height: 6,
    borderRadius: 3,
    backgroundColor: tokens.good,
  },
  footer: {
    width: '100%',
    alignItems: 'center',
  },
  vdisplayActions: {
    width: '100%',
    alignItems: 'center',
    gap: 8,
    marginBottom: 8,
  },
});

export default DisplaySheet;
