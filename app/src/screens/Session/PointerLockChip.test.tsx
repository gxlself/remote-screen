import { act, cleanup, render } from '@testing-library/react-native';
import { StyleSheet } from 'react-native';
import {
  pointerLockStore,
  setPointerLock,
} from '../../native/session/pointerLockStore';
import type { PointerLockState } from '../../native/session/pointerLockStore';
import { clearVideo, setViewport } from '../../features/session/videoStore';
import PointerLockChip from './PointerLockChip';
import type { SessionCtx } from './slots';

const state: PointerLockState = {
  mouse: true,
  locked: false,
  reason: 'notFullScreen',
};

function ctx(
  overrides: Partial<Pick<SessionCtx, 'sessionId' | 'isPad'>> = {},
): SessionCtx {
  return { sessionId: 's1', isPad: true, ...overrides } as SessionCtx;
}

async function show(
  value: PointerLockState,
  overrides: Partial<Pick<SessionCtx, 'sessionId' | 'isPad'>> = {},
) {
  await act(() => setPointerLock('s1', value));
  return render(<PointerLockChip ctx={ctx(overrides)} />);
}

beforeEach(() => {
  pointerLockStore.setState({ states: {} });
});

afterEach(async () => {
  await cleanup();
});

it('shows the split notice for an unlocked mouse on iPad', async () => {
  const view = await show(state);
  const chip = view.getByTestId('pointer-lock-chip');
  expect(view.getByText('分屏时不锁定指针 · 全屏后可锁定')).toBeOnTheScreen();
  expect(chip).toHaveProp(
    'accessibilityLabel',
    '分屏时不锁定指针，全屏后可锁定',
  );
  expect(chip).toHaveProp('accessible', true);
  expect(chip).toHaveProp('pointerEvents', 'none');
  expect(chip).toHaveStyle({
    top: 12,
    right: 59,
    height: 32,
    borderRadius: 16,
    backgroundColor: '#151A1E',
  });
  expect(
    StyleSheet.flatten(
      view.getByText('分屏时不锁定指针 · 全屏后可锁定').props.style,
    ),
  ).toMatchObject({ color: '#D5DCE0', fontSize: 12 });
});

it('shows the escaped notice after the pointer is released', async () => {
  const view = await show({ ...state, reason: 'escaped' });
  expect(view.getByText('指针已释放 · 点画面重新锁定')).toBeOnTheScreen();
  expect(view.getByTestId('pointer-lock-chip')).toHaveProp(
    'accessibilityLabel',
    '指针已释放，点画面重新锁定',
  );
});

it.each([
  ['locked', { ...state, locked: true }, {}],
  ['not iPad', state, { isPad: false }],
  ['no mouse', { ...state, mouse: false }, {}],
])('renders nothing when %s', async (_name, value, overrides) => {
  const view = await show(value, overrides);
  expect(view.toJSON()).toBeNull();
});

it('moves below the minimap while the picture is zoomed', async () => {
  await act(() => setPointerLock('s1', state));
  setViewport('s1', { zoom: 2, x: 0.5, y: 0.5 });
  try {
    const view = await render(
      <PointerLockChip
        ctx={
          {
            sessionId: 's1',
            isPad: true,
            keyboardOpen: false,
            view: { overlay: 'none' },
          } as SessionCtx
        }
      />,
    );
    expect(
      StyleSheet.flatten(view.getByTestId('pointer-lock-chip').props.style).top,
    ).toBe(12 + 63 + 8);
  } finally {
    clearVideo('s1');
  }
});
