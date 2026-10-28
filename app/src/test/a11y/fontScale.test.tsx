import { act, cleanup } from '@testing-library/react-native';
import { PixelRatio, StyleSheet } from 'react-native';
import ts from 'typescript';
import type { TabScreenProps } from '../../app/navigation/routes';
import { computers, FIXTURE_NOW } from '../../native/rscore/fixtures';
import Home from '../../screens/Home/index';
import { PHONE_WINDOW, mockPhoneDimensions } from '../dimensions';
import { mockNavigation } from '../navigation';
import { renderWithProviders } from '../render';
import { parseSource, sourceFiles } from './helpers';

// Only content previews may truncate. Names, headings and action text must wrap.
const ALLOWED_LINE_LIMITS = [
  {
    file: 'screens/SshKeys/KeyCard.tsx',
    content: '{key.publicKey}',
    reason: '公钥长串预览；复制 / 分享操作提供完整公钥。',
  },
  {
    file: 'screens/Session/ClipboardSheet.tsx',
    content: '{view.latestFromHost.text}',
    reason: '远端剪贴板长串预览；复制操作提供完整内容。',
  },
  {
    file: 'screens/Session/ClipboardSheet.tsx',
    content: '{item.text}',
    reason: '剪贴板历史长串预览；点按可复用完整内容。',
  },
];

function limitedElements(file: string, text?: string) {
  const source = parseSource(file, text);
  const elements: { line: number; text: string; content: string }[] = [];
  function visit(node: ts.Node) {
    if (
      ts.isJsxAttribute(node) &&
      node.name.getText(source) === 'numberOfLines'
    ) {
      const opening = node.parent.parent;
      const element = ts.isJsxOpeningElement(opening)
        ? opening.parent
        : opening;
      elements.push({
        line:
          source.getLineAndCharacterOfPosition(node.getStart(source)).line + 1,
        text: element.getText(source).replace(/\s+/g, ' '),
        content: ts.isJsxElement(element)
          ? element.children
              .map(child => child.getText(source))
              .join('')
              .trim()
          : '',
      });
    }
    ts.forEachChild(node, visit);
  }
  visit(source);
  return elements;
}

it('allows numberOfLines only on documented long-content previews', () => {
  const violations: string[] = [];
  const used = new Set<object>();
  for (const file of [
    ...sourceFiles('screens'),
    ...sourceFiles('components'),
  ]) {
    for (const element of limitedElements(file)) {
      const exception = ALLOWED_LINE_LIMITS.find(
        item => item.file === file && element.content === item.content,
      );
      if (exception !== undefined) {
        expect(exception.reason.trim()).not.toBe('');
        used.add(exception);
      } else {
        violations.push(
          `${file}:${element.line}: ${element.text.slice(0, 180)}`,
        );
      }
    }
  }
  expect(violations).toEqual([]);
  expect(ALLOWED_LINE_LIMITS.filter(item => !used.has(item))).toEqual([]);
});

it('does not extend a preview exception to sibling or nested titles', () => {
  const file = 'screens/Session/ClipboardSheet.tsx';
  const elements = limitedElements(
    file,
    [
      '<View>',
      '  <AppText numberOfLines={1}>{item.text}</AppText>',
      '  <AppText numberOfLines={1} />',
      '  <AppText numberOfLines={1}>Title</AppText>',
      '  <AppText numberOfLines={1}><AppText>{item.text}</AppText></AppText>',
      '</View>',
    ].join('\n'),
  );
  expect(elements).toHaveLength(4);
  expect(
    elements
      .filter(
        element =>
          !ALLOWED_LINE_LIMITS.some(
            item => item.file === file && element.content === item.content,
          ),
      )
      .map(element => element.line),
  ).toEqual([3, 4, 5]);
});

// This phone lists only the computers it has paired with.
const paired = computers.filter(computer => computer.paired);

it('uses one card per row and leaves every device name unlimited at font scale 3', async () => {
  jest.useFakeTimers();
  jest.setSystemTime(FIXTURE_NOW / 1000);
  const restore = mockPhoneDimensions();
  jest.spyOn(PixelRatio, 'getFontScale').mockReturnValue(3);
  let view: Awaited<ReturnType<typeof renderWithProviders>> | undefined;
  try {
    view = await renderWithProviders(
      <Home
        navigation={
          mockNavigation<'MainTabs'>() as unknown as TabScreenProps<'Home'>['navigation']
        }
        route={{ key: 'Home-fontScale', name: 'Home' }}
      />,
      { localDirectory: {} },
    );
    await act(() => jest.advanceTimersByTimeAsync(1));
    expect(view.getByTestId(`card-${computers[0].hostId}`)).toBeOnTheScreen();
    const cards = view.getAllByTestId(/^card-/);
    expect(cards).toHaveLength(paired.length);
    const rows = cards.map(card => card.parent?.parent);
    expect(new Set(rows).size).toBe(cards.length);
    for (const card of cards) {
      expect(StyleSheet.flatten(card.parent?.props.style)).toMatchObject({
        width: PHONE_WINDOW.width - 40,
      });
    }
    for (const computer of paired) {
      const names = view.getAllByText(computer.name);
      for (const name of names) {
        expect(name.type).toBe('Text');
        expect(name.props.numberOfLines).toBeUndefined();
      }
    }
  } finally {
    await cleanup();
    view?.queryClient.clear();
    restore();
    jest.restoreAllMocks();
    jest.useRealTimers();
  }
});

it('uses two cards per row with the source card gap at font scale 1', async () => {
  jest.useFakeTimers();
  jest.setSystemTime(FIXTURE_NOW / 1000);
  const restore = mockPhoneDimensions();
  jest.spyOn(PixelRatio, 'getFontScale').mockReturnValue(1);
  // Home/HomeNarrow.tsx keeps CARD_GAP private; mirror its 12 pt row gap.
  const CARD_GAP = 12;
  let view: Awaited<ReturnType<typeof renderWithProviders>> | undefined;
  try {
    view = await renderWithProviders(
      <Home
        navigation={
          mockNavigation<'MainTabs'>() as unknown as TabScreenProps<'Home'>['navigation']
        }
        route={{ key: 'Home-fontScale-default', name: 'Home' }}
      />,
      { localDirectory: {} },
    );
    await act(() => jest.advanceTimersByTimeAsync(1));
    expect(view.getByTestId(`card-${computers[0].hostId}`)).toBeOnTheScreen();
    const cards = view.getAllByTestId(/^card-/);
    expect(cards).toHaveLength(paired.length);
    const rows = cards.map(card => card.parent?.parent);
    expect(new Set(rows).size).toBe(Math.ceil(cards.length / 2));
    for (const card of cards) {
      expect(StyleSheet.flatten(card.parent?.props.style)).toMatchObject({
        width: (PHONE_WINDOW.width - 40 - CARD_GAP) / 2,
      });
    }
  } finally {
    await cleanup();
    view?.queryClient.clear();
    restore();
    jest.restoreAllMocks();
    jest.useRealTimers();
  }
});
