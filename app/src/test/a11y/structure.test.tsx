import { render } from '@testing-library/react-native';
import { Pressable, Text, View } from 'react-native';
import type { VideoSlotProps } from '../../screens/Session/slots';
import type { TerminalSlotProps } from '../../screens/Terminal/TerminalSlot';
import { swallowedControls, visibleElements, withoutAllowed } from './helpers';
import { cleanupA11y, screenCases, setupA11y } from './pages';

type SwallowException = {
  page: string;
  control: string;
  container: string;
  reason: string;
};
const ALLOWED_SWALLOW: SwallowException[] = [
  {
    page: 'Settings',
    control: '(无)',
    container: '光标速度 1.4× 慢：精细；快：快速划过时加速',
    reason:
      '读屏结构例外：光标速度整行提供 adjustable 的增减动作，两个视觉按钮由该元素统一读屏操作。',
  },
];
const PAGES_WITHOUT_HEADER: { page: string; reason: string }[] = [
  ...[
    'Session Mac toolbar phone',
    'Session Mac toolbar iPad',
    'Session Mac keyboard phone',
    'Session Mac keyboard iPad',
    'Session Windows toolbar phone',
    'Session Windows toolbar iPad',
  ].map(page => ({
    page,
    reason: '会话页的读屏焦点只在工具栏，见 07 · 无障碍',
  })),
  {
    page: 'Terminal',
    reason: '待修：终端页只有 tab 标题，需增加独立的 header。',
  },
];
const usedSwallow = new Set<SwallowException>();
const usedHeaders = new Set<(typeof PAGES_WITHOUT_HEADER)[number]>();

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
  expect(ALLOWED_SWALLOW.filter(item => !usedSwallow.has(item))).toEqual([]);
  expect(PAGES_WITHOUT_HEADER.filter(item => !usedHeaders.has(item))).toEqual(
    [],
  );
});

it.each(screenCases)('$name does not swallow controls', async item => {
  const view = await item.mount();
  const findings = swallowedControls(view.root).map(finding => ({
    page: item.name,
    ...finding,
  }));
  expect(withoutAllowed(findings, ALLOWED_SWALLOW, usedSwallow)).toEqual([]);
});

it.each(screenCases)('$name exposes a page or panel heading', async item => {
  const view = await item.mount();
  const scope =
    item.target === undefined ? view.root : view.getByTestId(item.target);
  const headers = visibleElements(scope).filter(
    node => node.props.accessibilityRole === 'header',
  );
  const findings = headers.length === 0 ? [{ page: item.name }] : [];
  expect(withoutAllowed(findings, PAGES_WITHOUT_HEADER, usedHeaders)).toEqual(
    [],
  );
});

it('finds Pressable nesting and accessible containers but ignores false and hidden containers', async () => {
  const view = await render(
    <View>
      <Pressable onPress={() => {}} testID="outer-pressable">
        <Pressable onPress={() => {}} testID="inner-pressable" />
      </Pressable>
      <View accessible testID="readable-container">
        <View accessibilityRole="button">
          <Text>Nested button</Text>
        </View>
      </View>
      <View accessible={false}>
        <Pressable onPress={() => {}} testID="reachable" />
      </View>
      <View accessible>
        <View accessibilityElementsHidden>
          <Pressable onPress={() => {}} />
        </View>
        <View importantForAccessibility="no-hide-descendants">
          <View accessibilityRole="button" />
        </View>
      </View>
    </View>,
  );
  expect(swallowedControls(view.root)).toEqual([
    { control: 'inner-pressable', container: 'outer-pressable' },
    { control: 'Nested button', container: 'readable-container' },
  ]);
});
