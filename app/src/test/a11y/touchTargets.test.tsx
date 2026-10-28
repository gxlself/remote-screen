import { render } from '@testing-library/react-native';
import { PixelRatio, Pressable, StyleSheet, Text, View } from 'react-native';
import type { VideoSlotProps } from '../../screens/Session/slots';
import type { TerminalSlotProps } from '../../screens/Terminal/TerminalSlot';
import {
  controlDescription,
  subtreeText,
  touchSize,
  visibleControls,
  withoutAllowed,
} from './helpers';
import type { ControlException } from './helpers';
import { cleanupA11y, screenCases, setupA11y } from './pages';

type SmallTarget = ControlException & {
  dimension: 'width' | 'height';
  size: number;
};
type FixedHeight = ControlException & { height: number };
const SMALL_TARGETS: SmallTarget[] = [];
const FIXED_HEIGHT_AT_LARGE_FONT: FixedHeight[] = [
  {
    page: 'Sessions',
    control: 'remote-thumb-s-live-1',
    height: 128,
    reason: '待修：缩略图按钮内有状态文字，字号 3 时仍固定高度。',
  },
];
const usedSmall = new Set<SmallTarget>();
const usedFixed = new Set<FixedHeight>();

jest.mock('../../screens/Session/VideoSlot', () => {
  const { View: MockView } =
    jest.requireActual<typeof import('react-native')>('react-native');
  function VideoSlot(props: VideoSlotProps) {
    return (
      <MockView
        accessible
        accessibilityRole="image"
        accessibilityLabel={props.accessibilityLabel}
        testID="a11y-video"
      />
    );
  }
  return { __esModule: true, default: VideoSlot, VideoSlot };
});
jest.mock('../../screens/Session/KeyboardSlot', () => {
  const { View: MockView } =
    jest.requireActual<typeof import('react-native')>('react-native');
  function KeyboardSlot() {
    return <MockView testID="a11y-keyboard" />;
  }
  return { __esModule: true, default: KeyboardSlot, KeyboardSlot };
});
jest.mock('../../screens/Terminal/TerminalSlot', () => {
  const { View: MockView } =
    jest.requireActual<typeof import('react-native')>('react-native');
  function TerminalSlot(props: TerminalSlotProps) {
    return (
      <MockView
        accessible
        accessibilityLabel={props.accessibilityLabel}
        testID="a11y-terminal"
      />
    );
  }
  return { TerminalSlot };
});

jest.setTimeout(30_000);
beforeEach(setupA11y);
afterEach(cleanupA11y);
afterAll(() => {
  expect(SMALL_TARGETS.filter(item => !usedSmall.has(item))).toEqual([]);
  expect(
    FIXED_HEIGHT_AT_LARGE_FONT.filter(item => !usedFixed.has(item)),
  ).toEqual([]);
});

it.each(screenCases)(
  '$name has at least 44 pt in every explicit touch dimension',
  async item => {
    const view = await item.mount();
    const findings: Omit<SmallTarget, 'reason'>[] = [];
    for (const node of visibleControls(view.root)) {
      const size = touchSize(node);
      for (const dimension of ['width', 'height'] as const) {
        const value = size[dimension];
        if (value !== undefined && value < 44) {
          findings.push({
            page: item.name,
            control: controlDescription(node),
            dimension,
            size: value,
          });
        }
      }
    }
    expect(withoutAllowed(findings, SMALL_TARGETS, usedSmall)).toEqual([]);
  },
);

it.each(screenCases)(
  '$name lets text actions grow at font scale 3',
  async item => {
    jest.spyOn(PixelRatio, 'getFontScale').mockReturnValue(3);
    const view = await item.mount();
    const findings = visibleControls(view.root).flatMap(node => {
      const height = StyleSheet.flatten(node.props.style)?.height;
      return subtreeText(node) !== '' && typeof height === 'number'
        ? [{ page: item.name, control: controlDescription(node), height }]
        : [];
    });
    expect(
      withoutAllowed(findings, FIXED_HEIGHT_AT_LARGE_FONT, usedFixed),
    ).toEqual([]);
  },
);

it('measures numeric dimensions, minimums, flattened overrides and both forms of hitSlop', async () => {
  const view = await render(
    <View>
      <Pressable
        testID="small"
        onPress={() => {}}
        style={{ width: 32, height: 30 }}
      />
      <Pressable
        testID="number-slop"
        onPress={() => {}}
        style={{ width: 32, height: 32 }}
        hitSlop={6}
      />
      <Pressable
        testID="edge-slop"
        onPress={() => {}}
        style={[
          { width: 30, height: 32 },
          { minWidth: 36, minHeight: 40 },
        ]}
        hitSlop={{ left: 3, right: 5, top: 1, bottom: 3 }}
      />
      <Pressable
        testID="unknown"
        onPress={() => {}}
        style={{ width: '100%', flex: 1 }}
        hitSlop={20}
      />
      <Pressable
        testID="grow"
        onPress={() => {}}
        style={[{ height: 44 }, { height: undefined, minHeight: 44 }]}
      >
        <Text>Wrap me</Text>
      </Pressable>
      <View
        accessible={false}
        style={{ width: 30, height: 30 }}
        testID="follow-zoom-like"
      />
    </View>,
  );
  expect(touchSize(view.getByTestId('small'))).toEqual({
    width: 32,
    height: 30,
  });
  expect(touchSize(view.getByTestId('number-slop'))).toEqual({
    width: 44,
    height: 44,
  });
  expect(touchSize(view.getByTestId('edge-slop'))).toEqual({
    width: 44,
    height: 44,
  });
  expect(touchSize(view.getByTestId('unknown'))).toEqual({
    width: undefined,
    height: undefined,
  });
  expect(touchSize(view.getByTestId('grow'))).toEqual({
    width: undefined,
    height: 44,
  });
  expect(visibleControls(view.root).map(controlDescription)).not.toContain(
    'follow-zoom-like',
  );
  const small = visibleControls(view.root).filter(node => {
    const size = touchSize(node);
    return (
      (size.width !== undefined && size.width < 44) ||
      (size.height !== undefined && size.height < 44)
    );
  });
  expect(small.map(controlDescription)).toEqual(['small']);
});

it('detects a fixed-height text button and permits the height override at large font scale', async () => {
  jest.spyOn(PixelRatio, 'getFontScale').mockReturnValue(3);
  const view = await render(
    <View>
      <Pressable testID="fixed" onPress={() => {}} style={{ height: 44 }}>
        <Text>Clipped</Text>
      </Pressable>
      <Pressable
        testID="flexible"
        onPress={() => {}}
        style={[
          { height: 44 },
          PixelRatio.getFontScale() > 2
            ? { height: undefined, minHeight: 44 }
            : null,
        ]}
      >
        <Text>Wrapping</Text>
      </Pressable>
    </View>,
  );
  expect(
    visibleControls(view.root)
      .filter(
        node =>
          subtreeText(node) !== '' &&
          typeof StyleSheet.flatten(node.props.style)?.height === 'number',
      )
      .map(controlDescription),
  ).toEqual(['fixed']);
});
