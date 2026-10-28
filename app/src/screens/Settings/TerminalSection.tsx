import { StyleSheet, View } from 'react-native';
import { ListGroup, ListRow, SectionHeader } from '../../components/List';
import { Switch } from '../../components/Switch';
import { useSetting } from '../../features/settings/store';
import { useSnippets } from '../../features/terminal/useSnippets';
import { settings as copy } from '../../i18n/zh/settings';
import { space } from '../../theme/tokens';
import { SnippetsEditor } from '../Terminal/SnippetsEditor';
import { OptionSheet } from './OptionSheet';

// T26: up to 20 for reading on a phone held at arm's length.
const FONT_SIZES = [11, 12, 13, 14, 15, 16, 17, 18, 20];
const FONT_OPTIONS = FONT_SIZES.map(size => ({
  value: size,
  label: String(size),
}));

type Props = {
  fontSheetOpen: boolean;
  snippetsOpen: boolean;
  onOpenFontSheet: () => void;
  onCloseFontSheet: () => void;
  onOpenSnippets: () => void;
  onCloseSnippets: () => void;
};

export function TerminalSection({
  fontSheetOpen,
  snippetsOpen,
  onOpenFontSheet,
  onCloseFontSheet,
  onOpenSnippets,
  onCloseSnippets,
}: Props) {
  const [fontSizeValue, setFontSizeRaw] = useSetting('terminal.fontSize');
  const fontSize: number = fontSizeValue;
  const setFontSize = setFontSizeRaw as (value: number) => void;
  const [tmux, setTmux] = useSetting('terminal.tmux');
  const snippets = useSnippets();

  return (
    <View style={styles.section}>
      <SectionHeader title={copy.terminal.section} />
      <ListGroup>
        <ListRow
          chevron
          onPress={onOpenFontSheet}
          title={copy.terminal.font.title}
          value={copy.terminal.font.value(fontSize)}
        />
        {/* S20: fixed, so no value column that reads like a choice. */}
        <ListRow
          subtitle={copy.terminal.colorScheme.subtitle}
          title={copy.terminal.colorScheme.title}
        />
        <ListRow
          right={
            <Switch
              accessibilityHint={copy.terminal.tmux.subtitle}
              accessibilityLabel={copy.terminal.tmux.title}
              onValueChange={setTmux}
              value={tmux}
            />
          }
          subtitle={copy.terminal.tmux.subtitle}
          title={copy.terminal.tmux.title}
        />
        <ListRow
          chevron
          onPress={onOpenSnippets}
          title={copy.terminal.snippets.title}
          value={copy.terminal.snippets.count(snippets.items.length)}
        />
      </ListGroup>
      {fontSheetOpen ? (
        <OptionSheet
          onClose={onCloseFontSheet}
          onSelect={setFontSize}
          options={FONT_OPTIONS}
          title={copy.terminal.font.sheetTitle}
          value={fontSize}
        />
      ) : null}
      {snippetsOpen ? <SnippetsEditor onClose={onCloseSnippets} /> : null}
    </View>
  );
}

const styles = StyleSheet.create({
  section: {
    gap: space.sm,
  },
});

export default TerminalSection;
