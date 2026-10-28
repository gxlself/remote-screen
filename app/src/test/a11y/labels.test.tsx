import {
  act,
  cleanup,
  fireEvent,
  render,
  waitFor,
} from '@testing-library/react-native';
import { AccessibilityInfo, Pressable, Text, View } from 'react-native';
import ts from 'typescript';
import { resetSshStore } from '../../features/sessions/sshStore';
import { createFakeCore } from '../../native/rscore/fakeCore';
import {
  computers,
  FIXTURE_NOW,
  liveSession,
} from '../../native/rscore/fixtures';
import type { CoreEventOf } from '../../native/rscore/types';
import Home from '../../screens/Home/index';
import Session from '../../screens/Session/index';
import type { VideoSlotProps } from '../../screens/Session/slots';
import SessionEnded from '../../screens/SessionEnded/index';
import type { TerminalSlotProps } from '../../screens/Terminal/TerminalSlot';
import { mockPhoneDimensions, mockWindowDimensions } from '../dimensions';
import {
  expectLabels,
  interactiveElements,
  missingLabels,
  parseSource,
  sourceFiles,
} from './helpers';

import {
  clients,
  mount,
  pages,
  rootPage,
  tabPage,
  sessionCases,
  overlayCases,
} from './pages';

jest.setTimeout(30_000);

const mockVideoProps = jest.fn<void, [VideoSlotProps]>();

jest.mock('../../screens/Session/VideoSlot', () => {
  const { View: MockView } =
    jest.requireActual<typeof import('react-native')>('react-native');
  function VideoSlot(props: VideoSlotProps) {
    mockVideoProps(props);
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

const studio = computers[0];
const office = computers[1];
let restoreDimensions: () => void;

beforeEach(() => {
  jest.useFakeTimers();
  jest.setSystemTime(FIXTURE_NOW / 1000);
  restoreDimensions = mockPhoneDimensions();
  resetSshStore();
  mockVideoProps.mockClear();
});

afterEach(async () => {
  await cleanup();
  clients.splice(0).forEach(client => client.clear());
  resetSshStore();
  restoreDimensions();
  jest.restoreAllMocks();
  jest.useRealTimers();
});

it.each(pages)(
  '$name gives every interactive element a readable name',
  async ({ ui, ready, wide, options }) => {
    if (wide) {
      restoreDimensions();
      restoreDimensions = mockWindowDimensions({ width: 1024, height: 768 });
    }
    const view = await mount(ui(), options?.());
    await view.findAllByText(ready);
    expectLabels(view.root);
  },
);

it('keeps IconButton labels in the locale a11y sections', () => {
  const violations: string[] = [];
  let buttons = 0;
  for (const file of [
    ...sourceFiles('screens'),
    ...sourceFiles('components'),
  ]) {
    const source = parseSource(file);
    function visit(node: ts.Node) {
      if (
        (ts.isJsxOpeningElement(node) || ts.isJsxSelfClosingElement(node)) &&
        node.tagName.getText(source) === 'IconButton'
      ) {
        buttons += 1;
        const label = node.attributes.properties.find(
          attribute =>
            ts.isJsxAttribute(attribute) &&
            attribute.name.getText(source) === 'accessibilityLabel',
        );
        if (label === undefined || !/\ba11y\b/.test(label.getText(source))) {
          const { line } = source.getLineAndCharacterOfPosition(
            node.getStart(source),
          );
          violations.push(
            `${file}:${line + 1}: ${
              label?.getText(source) ?? 'missing accessibilityLabel'
            }`,
          );
        }
      }
      ts.forEachChild(node, visit);
    }
    visit(source);
  }
  expect(buttons).toBeGreaterThan(0);
  expect(violations).toEqual([]);
});

it.each([false, true])(
  'labels the search-clear icon on Home (wide=%s)',
  async wide => {
    if (wide) {
      restoreDimensions();
      restoreDimensions = mockWindowDimensions({ width: 1024, height: 768 });
    }
    const view = await mount(tabPage(Home, 'Home'));
    await view.findAllByText(studio.name);
    await fireEvent.changeText(view.getByLabelText('搜索设备'), 'Studio');
    expect(await view.findByTestId('home-search-clear')).toHaveProp(
      'accessibilityLabel',
      '清除搜索',
    );
    expectLabels(view.root);
  },
);

// Ten named error boards, plus their App-version / FileVault / generic variants.
it.each([
  101, 110, 120, 130, 140, 150, 160, 161, 170, 190, 171, 191, 180, 181, 142,
])('SessionEnded E%s gives every action a readable name', async code => {
  const view = await mount(
    rootPage(SessionEnded, 'SessionEnded', { hostId: studio.hostId, code }),
  );
  await view.findAllByText(
    code === 180 ? '这台设备已被作废' : /Studio Mac mini/,
  );
  expectLabels(view.root);
});

async function sessionView(wide = false) {
  if (wide) {
    restoreDimensions();
    restoreDimensions = mockWindowDimensions({ width: 1024, height: 768 });
  }
  const view = await mount(
    rootPage(Session, 'Session', {
      hostId: studio.hostId,
      sessionId: liveSession.id,
    }),
  );
  await view.findByTestId('toolbar-disconnect');
  await act(() =>
    view.core.emit({
      type: 'session.displays',
      sessionId: liveSession.id,
      displays: studio.displays,
      currentId: 1,
    }),
  );
  return view;
}

it.each(
  [
    { panel: 'toolbar', trigger: null, target: 'layer-toolbar' },
    { panel: 'quality', trigger: 'toolbar-quality', target: 'quality-sheet' },
    { panel: 'displays', trigger: 'toolbar-displays', target: 'display-sheet' },
    {
      panel: 'clipboard',
      trigger: 'toolbar-clipboard',
      target: 'clipboard-sheet',
    },
    { panel: 'more', trigger: 'toolbar-more', target: 'more-sheet' },
    { panel: 'keyboard', trigger: 'toolbar-keyboard', target: 'a11y-keyboard' },
    { panel: 'coach', trigger: 'toolbar-more', target: 'gesture-coach' },
  ].flatMap(panel => [
    { ...panel, wide: false, size: 'phone' },
    { ...panel, wide: true, size: 'iPad' },
  ]),
)(
  'Session Mac controller labels the $panel panel on $size',
  async ({ panel, trigger, target, wide }) => {
    const view = await sessionView(wide);
    if (panel === 'clipboard') {
      await act(() => {
        view.core.emit({
          type: 'session.clipboard',
          sessionId: liveSession.id,
          direction: 'toHost',
          text: 'pwd',
          at: FIXTURE_NOW - 2_000_000,
        });
        view.core.emit({
          type: 'session.clipboard',
          sessionId: liveSession.id,
          direction: 'fromHost',
          text: 'hostname',
          at: FIXTURE_NOW - 1_000_000,
        });
      });
    }
    if (trigger !== null) {
      if (panel === 'clipboard' && !wide) {
        await fireEvent.press(view.getByTestId('toolbar-more'));
        await view.findByTestId('more-sheet');
        await fireEvent.press(view.getByRole('button', { name: '剪贴板' }));
      } else {
        await fireEvent.press(view.getByTestId(trigger));
      }
      if (panel === 'coach') {
        await view.findByTestId('more-sheet');
        await fireEvent.press(view.getByRole('button', { name: '手势说明' }));
      }
    }
    const contents = await view.findByTestId(target);
    // The native keyboard is deliberately a plain View mock; its controls belong
    // to P5. Every JS panel must contain and label its own interactive elements.
    if (panel !== 'keyboard') {
      expectLabels(contents);
    }
    if (panel === 'clipboard') {
      await view.findByTestId('clipboard-recent');
    }
    expectLabels(view.root);
  },
);

function pathStats(
  sessionId: string,
  relay = false,
): CoreEventOf<'session.stats'> {
  return {
    type: 'session.stats',
    sessionId,
    rttMs: relay ? 48 : 6,
    fps: 60,
    bitrateKbps: 8000,
    decodeMs: 3,
    lossPct: 0,
    width: 2560,
    height: 1600,
    codec: 'hevc',
    path: relay ? 'tailnetDerp' : 'tailnetDirect',
    transport: 'quic',
    ...(relay ? { derpRegion: '上海' } : {}),
  };
}

it('matches the Studio Mac mini A11y sample and double-tap / long-press hint', async () => {
  const view = await mount(tabPage(Home, 'Home'));
  await view.findAllByText(studio.name);
  await act(() => view.core.emit(pathStats(liveSession.id)));
  const card = view.getByTestId(`card-${studio.hostId}`);
  expect(card).toHaveProp(
    'accessibilityLabel',
    // H13: the card's chip (会话中) is read out too.
    'Studio Mac mini，直连 6 毫秒，2 块屏幕，会话中',
  );
  expect(card).toHaveProp('accessibilityHint', '轻点两下开始远控，长按看详情');
  expect(card).toHaveProp('accessibilityRole', 'button');
});

it('matches the Windows relay A11y sample after session.stats', async () => {
  const core = createFakeCore({
    sessions: [
      liveSession,
      {
        ...liveSession,
        id: 'office-a11y',
        hostId: office.hostId,
        hostName: office.name,
      },
    ],
  });
  const view = await mount(tabPage(Home, 'Home'), { core });
  await view.findAllByText(office.name);
  await act(() => core.emit(pathStats('office-a11y', true)));
  expect(view.getByTestId(`card-${office.hostId}`)).toHaveProp(
    'accessibilityLabel',
    // H13: the card's chip (会话中) is read out too.
    '工位 Windows，经上海中继，48 毫秒，会话中',
  );
});

it('names the add-device icon button as in the A11y sample', async () => {
  const view = await mount(tabPage(Home, 'Home'));
  await view.findAllByText(studio.name);
  expect(view.getByRole('button', { name: '添加设备' })).toHaveProp(
    'accessibilityLabel',
    '添加设备',
  );
});

it('exposes the remote picture as one accessible element and forwards reduceMotion', async () => {
  jest
    .spyOn(AccessibilityInfo, 'isReduceMotionEnabled')
    .mockResolvedValue(true);
  const view = await sessionView();
  const picture = view.getByTestId('a11y-video');
  expect(picture).toHaveProp('accessible', true);
  expect(picture).toHaveProp(
    'accessibilityLabel',
    'Studio Mac mini 远端画面，双指双击切换直接触控',
  );
  expect(picture.children).toHaveLength(0);
  await waitFor(() =>
    expect(mockVideoProps).toHaveBeenLastCalledWith(
      expect.objectContaining({ reduceMotion: true }),
    ),
  );
  expect(view.getByTestId('layer-toolbar')).toBeOnTheScreen();
});

it('detects unnamed icons, empty labels and hidden-text fallbacks', async () => {
  const view = await render(
    <View>
      <View accessibilityRole="button" testID="unnamed-icon" />
      <View
        accessibilityRole="switch"
        accessibilityLabel=" "
        testID="empty-label"
      />
      <Pressable onPress={() => {}} testID="hidden-text">
        <View accessibilityElementsHidden>
          <Text>hidden</Text>
        </View>
      </Pressable>
      <View accessibilityRole="button">
        <Text>Readable text</Text>
      </View>
      <Pressable onPress={() => {}} accessible={false} testID="non-element" />
      <View accessibilityRole="radio" accessibilityLabel="Radio" />
      <View accessibilityRole="tab" accessibilityLabel="Tab" />
      <View accessibilityRole="link" accessibilityLabel="Link" />
      <View accessibilityRole="adjustable" accessibilityLabel="Slider" />
    </View>,
  );
  expect(interactiveElements(view.root)).toHaveLength(8);
  expect(missingLabels(view.root)).toEqual([
    'unnamed-icon (button)',
    'empty-label (switch)',
    'hidden-text (onPress)',
  ]);
});

it.each(sessionCases.filter(item => item.name.startsWith('Session Windows')))(
  '$name gives the toolbar and panel readable names',
  async item => {
    const view = await item.mount();
    expectLabels(await view.findByTestId(item.target));
    expectLabels(view.root);
  },
);

it.each(overlayCases)(
  '$name labels every available action',
  async ({ ui, ready }) => {
    const view = await mount(ui());
    await view.findAllByText(ready);
    expectLabels(view.root);
  },
);
