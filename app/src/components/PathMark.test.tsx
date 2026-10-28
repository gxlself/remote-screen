import { render, screen } from '@testing-library/react-native';
import { StyleSheet } from 'react-native';
import { PathLabel, PathMark, TcpTag, pathSpoken } from './PathMark';

function markStyle() {
  return StyleSheet.flatten(screen.getByTestId('mark').props.style);
}

describe('PathMark', () => {
  it.each([
    ['tailnetDirect', 7, 3.5, '#5DD39A'],
    ['tailnetDerp', 7, 1.5, '#F4B24E'],
    ['intranet', 7, 2, '#7DA8FF'],
    ['lan', 7, 2, '#7DA8FF'],
    ['manual', 7, 2, '#7DA8FF'],
  ] as const)(
    'renders %s with the canvas colour and radius',
    async (kind, size, borderRadius, color) => {
      await render(<PathMark kind={kind} testID="mark" />);
      expect(markStyle()).toMatchObject({
        width: size,
        height: size,
        borderRadius,
        backgroundColor: color,
      });
    },
  );

  it('rotates the relay mark 45 degrees', async () => {
    await render(<PathMark kind="tailnetDerp" testID="mark" />);
    expect(markStyle().transform).toEqual([{ rotate: '45deg' }]);
  });

  it('draws the offline mark as a larger dashed outline', async () => {
    await render(<PathMark kind="offline" testID="mark" />);
    expect(markStyle()).toMatchObject({
      width: 9,
      height: 9,
      borderRadius: 2,
      borderWidth: 1.3,
      borderStyle: 'dashed',
      borderColor: '#86929A',
    });
    expect(markStyle().backgroundColor).toBeUndefined();
  });

  it('keeps the shape out of the accessibility tree', async () => {
    await render(<PathMark kind="tailnetDirect" testID="mark" />);
    expect(screen.getByTestId('mark')).toHaveProp('accessible', false);
  });

  it('renders the mono TCP tag', async () => {
    await render(<TcpTag />);
    expect(screen.getByText('TCP')).toBeOnTheScreen();
    expect(
      StyleSheet.flatten(screen.getByText('TCP').props.style).fontFamily,
    ).toMatch(/^GeistMono-/);
  });
});

describe('PathLabel', () => {
  it('renders the path name and formatted latency', async () => {
    await render(<PathLabel kind="tailnetDirect" latencyMs={6} />);
    expect(screen.getByText('直连')).toBeOnTheScreen();
    expect(screen.getByText('6 ms')).toBeOnTheScreen();
    expect(screen.getByLabelText('直连 6 毫秒')).toBeOnTheScreen();
  });

  it('renders the TCP tag when requested', async () => {
    await render(<PathLabel kind="tailnetDerp" latencyMs={48} tcp />);
    expect(screen.getByText('中继')).toBeOnTheScreen();
    expect(screen.getByText('TCP')).toBeOnTheScreen();
  });

  it('allows callers to replace the written path', async () => {
    await render(
      <PathLabel kind="intranet" text="经 jump-01" latencyMs={12} />,
    );
    expect(screen.getByText('经 jump-01')).toBeOnTheScreen();
    expect(screen.getByLabelText('经 jump-01 12 毫秒')).toBeOnTheScreen();
  });

  it('uses the muted latency colour by default and tints it on request', async () => {
    await render(<PathLabel kind="tailnetDirect" latencyMs={6} />);
    expect(StyleSheet.flatten(screen.getByText('6 ms').props.style).color).toBe(
      '#ECF0F2',
    );

    await screen.unmount();
    await render(<PathLabel kind="tailnetDirect" latencyMs={6} tintLatency />);
    expect(StyleSheet.flatten(screen.getByText('6 ms').props.style).color).toBe(
      '#5DD39A',
    );
  });

  it('keeps relay latency amber and uses HUD colours only when tinted', async () => {
    await render(<PathLabel kind="tailnetDerp" latencyMs={48} />);
    expect(
      StyleSheet.flatten(screen.getByText('48 ms').props.style).color,
    ).toBe('#F4B24E');

    await screen.unmount();
    await render(<PathLabel kind="intranet" latencyMs={44} />);
    expect(
      StyleSheet.flatten(screen.getByText('44 ms').props.style).color,
    ).toBe('#ECF0F2');

    await screen.unmount();
    await render(<PathLabel kind="intranet" latencyMs={44} tintLatency />);
    expect(
      StyleSheet.flatten(screen.getByText('44 ms').props.style).color,
    ).toBe('#A9C3FF');
  });

  it('uses the small type sizes when requested', async () => {
    await render(<PathLabel kind="lan" latencyMs={5} size="sm" />);
    expect(
      StyleSheet.flatten(screen.getByText('局域网').props.style).fontSize,
    ).toBe(12);
    expect(
      StyleSheet.flatten(screen.getByText('5 ms').props.style).fontSize,
    ).toBe(12);
  });
});

describe('pathSpoken', () => {
  it('speaks latency and the offline state', () => {
    expect(pathSpoken('tailnetDirect', 6)).toBe('直连 6 毫秒');
    expect(pathSpoken('offline')).toBe('离线');
    expect(pathSpoken('tailnetDerp')).toBe('中继');
  });
});
