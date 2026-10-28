import { render, screen } from '@testing-library/react-native';
import { StyleSheet } from 'react-native';
import { StatusChip, type StatusChipTone } from './StatusChip';

const tones: Array<
  [StatusChipTone, string | undefined, string, string | undefined]
> = [
  ['good', 'rgba(93, 211, 154, 0.14)', '#5DD39A', undefined],
  ['relay', 'rgba(244, 178, 78, 0.14)', '#F4B24E', undefined],
  ['neutral', '#1C2328', '#A9B4BB', undefined],
  ['offline', undefined, '#86929A', 'dashed'],
  ['hostControlled', '#FF9F43', '#2A1500', undefined],
  ['track', '#252D33', '#A9B4BB', undefined],
  ['accent', 'rgba(125, 168, 255, 0.16)', '#7DA8FF', undefined],
];

describe('StatusChip', () => {
  it.each(tones)(
    'renders the %s tone with its canvas colours',
    async (tone, background, color, borderStyle) => {
      await render(<StatusChip label={tone} tone={tone} testID="chip" />);
      const chip = screen.getByTestId('chip');
      expect(StyleSheet.flatten(chip.props.style)).toMatchObject({
        backgroundColor: background,
        ...(borderStyle ? { borderStyle } : {}),
      });
      expect(StyleSheet.flatten(screen.getByText(tone).props.style).color).toBe(
        color,
      );
      expect(chip).toHaveProp('accessibilityLabel', tone);
    },
  );

  it('uses the 26 px medium and 20 px small metrics', async () => {
    await render(<StatusChip label="在线" tone="good" testID="chip" />);
    expect(
      StyleSheet.flatten(screen.getByTestId('chip').props.style),
    ).toMatchObject({
      minHeight: 26,
      borderRadius: 13,
      paddingHorizontal: 10,
      gap: 6,
    });

    await screen.unmount();
    await render(
      <StatusChip label="在线" tone="good" size="sm" testID="chip" />,
    );
    expect(
      StyleSheet.flatten(screen.getByTestId('chip').props.style),
    ).toMatchObject({
      minHeight: 20,
      borderRadius: 10,
      paddingHorizontal: 7,
      gap: 4,
    });
  });

  it('renders a path mark when one is supplied', async () => {
    await render(
      <StatusChip
        label="直连"
        tone="good"
        mark="tailnetDirect"
        testID="chip"
      />,
    );
    expect(screen.getByTestId('chip')).toHaveTextContent('直连');
    expect(
      screen
        .getByTestId('chip')
        .queryAll(node => node.props.accessible === false),
    ).toHaveLength(1);
  });
});
