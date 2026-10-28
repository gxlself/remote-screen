import type { ReactNode } from 'react';
import { StyleSheet, View } from 'react-native';
import AppText from '../../components/AppText';
import { Icon } from '../../components/icons/Icon';
import { sessionEnded } from '../../i18n/zh/sessionEnded';
import { tokens } from '../../theme/tokens';

export type SummaryRow = { label: string; value?: string };

export function summaryText(rows: SummaryRow[], notes?: string[]): string {
  const parts = rows.map(row =>
    row.value === undefined
      ? row.label
      : sessionEnded.a11y.pair(row.label, row.value),
  );
  return sessionEnded.a11y.join([...parts, ...(notes ?? [])]);
}

export type MockWindowProps = {
  title: string;
  children: ReactNode;
  dots?: boolean;
};

export function MockWindow({ title, children, dots = true }: MockWindowProps) {
  return (
    <View style={styles.window}>
      <View style={styles.windowBar}>
        {dots ? (
          <View style={styles.dots}>
            <View style={[styles.dot, styles.dotRed]} />
            <View style={[styles.dot, styles.dotYellow]} />
            <View style={[styles.dot, styles.dotGreen]} />
          </View>
        ) : null}
        <AppText color="text2" style={styles.windowTitle}>
          {title}
        </AppText>
      </View>
      <View style={styles.windowBody}>{children}</View>
    </View>
  );
}

export type StatusRowProps = {
  label: string;
  value?: string;
  tone?: 'good' | 'bad' | 'relay' | 'muted';
  mono?: boolean;
};

const toneColors = {
  good: 'good',
  bad: 'bad',
  relay: 'relay',
  muted: 'text3',
} as const;

export function StatusRow({
  label,
  value,
  tone = 'muted',
  mono = false,
}: StatusRowProps) {
  const color = toneColors[tone];
  return (
    <View style={styles.statusRow}>
      <AppText color="text2" style={styles.statusText}>
        {label}
      </AppText>
      {value === undefined ? null : (
        <View style={styles.statusValue}>
          {tone === 'good' ? (
            <Icon color={color} name="check" size={12} />
          ) : tone === 'bad' ? (
            <Icon color={color} name="close" size={12} />
          ) : null}
          <AppText color={color} mono={mono} style={styles.statusText}>
            {value}
          </AppText>
        </View>
      )}
    </View>
  );
}

export type ChipPathProps = {
  items: string[];
  accessibilityLabel: string;
};

export function ChipPath({ items, accessibilityLabel }: ChipPathProps) {
  return (
    <View
      accessible
      accessibilityLabel={accessibilityLabel}
      style={styles.path}
    >
      {items.map((item, index) => (
        <View key={`${item}-${index}`} style={styles.pathPart}>
          {index > 0 ? (
            <AppText color="muted" style={styles.pathSeparator}>
              ›
            </AppText>
          ) : null}
          <AppText
            style={styles.pathText}
            weight={index === items.length - 1 ? 600 : undefined}
          >
            {item}
          </AppText>
        </View>
      ))}
    </View>
  );
}

export type CodeBlockProps = { text: string };

export function CodeBlock({ text }: CodeBlockProps) {
  return (
    <View style={styles.codeBlock}>
      <AppText mono style={styles.codeText}>
        {text}
      </AppText>
    </View>
  );
}

export type NoteBoxProps = {
  text: string;
  tone?: 'accent' | 'relay';
};

export function NoteBox({ text, tone = 'accent' }: NoteBoxProps) {
  return (
    <View style={[styles.note, tone === 'relay' ? styles.noteRelay : null]}>
      <AppText style={styles.noteText}>{text}</AppText>
    </View>
  );
}

const styles = StyleSheet.create({
  window: {
    overflow: 'hidden',
    borderRadius: 18,
    backgroundColor: tokens.card,
    borderWidth: 1,
    borderColor: tokens.cardLine,
  },
  windowBar: {
    minHeight: 30,
    flexDirection: 'row',
    alignItems: 'center',
    backgroundColor: tokens.raised,
    paddingHorizontal: 10,
  },
  dots: { flexDirection: 'row', gap: 5 },
  dot: { width: 9, height: 9, borderRadius: 4.5 },
  dotRed: { backgroundColor: '#EC6A5E' },
  dotYellow: { backgroundColor: '#F4BF4F' },
  dotGreen: { backgroundColor: '#61C554' },
  windowTitle: {
    flex: 1,
    textAlign: 'center',
    fontSize: 11,
    lineHeight: 14,
  },
  windowBody: { padding: 12, gap: 8 },
  statusRow: {
    minHeight: 32,
    flexDirection: 'row',
    flexWrap: 'wrap',
    alignItems: 'center',
    justifyContent: 'space-between',
    gap: 8,
  },
  statusText: { fontSize: 11.5, lineHeight: 16 },
  statusValue: { flexDirection: 'row', alignItems: 'center', gap: 5 },
  path: {
    minHeight: 26,
    flexDirection: 'row',
    flexWrap: 'wrap',
    alignItems: 'center',
    alignSelf: 'flex-start',
    borderRadius: 8,
    backgroundColor: tokens.raised,
    paddingHorizontal: 8,
    paddingVertical: 4,
  },
  pathPart: { flexDirection: 'row', alignItems: 'center', gap: 5 },
  pathSeparator: { fontSize: 11, lineHeight: 14 },
  pathText: { fontSize: 11, lineHeight: 14 },
  codeBlock: {
    borderRadius: 10,
    backgroundColor: tokens.raised,
    padding: 10,
  },
  codeText: {
    fontSize: 11,
    lineHeight: 16,
    fontFamily: 'GeistMono-Regular',
  },
  note: {
    borderRadius: 10,
    backgroundColor: tokens.accentTint,
    paddingHorizontal: 10,
    paddingVertical: 8,
  },
  noteRelay: { backgroundColor: tokens.accentTint },
  noteText: {
    color: '#A9C3FF',
    fontSize: 11.5,
    lineHeight: 16,
  },
});
