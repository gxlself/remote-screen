import { cleanup, screen } from '@testing-library/react-native';
import { PixelRatio, Platform, StyleSheet } from 'react-native';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { FIXTURE_NOW } from '../../native/rscore/fixtures';
import { mockNavigation, mockRoute } from '../../test/navigation';
import { renderWithProviders } from '../../test/render';
import type { RootScreenProps } from '../../app/navigation/routes';
import DeviceSheetScreen from '.';

const STUDIO_HOST_ID = 'h7v2m9q3k8x4tb1a';
const OFFICE_HOST_ID = 'w3k8p2x7m4q9tb6c';

function route(
  hostId = STUDIO_HOST_ID,
): RootScreenProps<'DeviceSheet'>['route'] {
  return mockRoute('DeviceSheet', { hostId });
}

async function renderSheet(hostId = STUDIO_HOST_ID) {
  const rendered = await renderWithProviders(
    <DeviceSheetScreen navigation={mockNavigation()} route={route(hostId)} />,
    { core: createFakeCore(), localDirectory: { peers: [] } },
  );
  const title = hostId === STUDIO_HOST_ID ? 'Studio Mac mini' : '工位 Windows';
  await screen.findByText(title);
  return rendered;
}

beforeEach(() => {
  jest.useFakeTimers();
  jest.setSystemTime(new Date(FIXTURE_NOW / 1000));
});

afterEach(() => {
  jest.restoreAllMocks();
  jest.useRealTimers();
});

it('explains why the lock quick action is disabled', async () => {
  await renderSheet(OFFICE_HOST_ID);

  expect(screen.getByTestId('quick-action-lock')).toHaveProp(
    'accessibilityHint',
    '有会话进行中时才能锁定',
  );
});

it('wraps quick actions and stacks display options at the largest font size', async () => {
  jest.spyOn(PixelRatio, 'getFontScale').mockReturnValue(3);
  await renderSheet();

  expect(
    StyleSheet.flatten(screen.getByTestId('quick-actions').props.style),
  ).toMatchObject({ flexWrap: 'wrap' });
  expect(
    StyleSheet.flatten(
      screen.getByTestId('quick-action-view-only').props.style,
    ),
  ).toMatchObject({ flexBasis: '47%' });
  expect(
    StyleSheet.flatten(screen.getByTestId('display-option-1').props.style),
  ).toMatchObject({ flexBasis: '100%' });
});

it('labels the display grid as a radio group', async () => {
  await renderSheet();

  expect(screen.getByTestId('display-grid')).toHaveProp(
    'accessibilityRole',
    'radiogroup',
  );
  expect(screen.getByTestId('display-grid')).toHaveProp(
    'accessibilityLabel',
    '选择显示器',
  );
});

it('adds Android ripple to display and quick actions only on Android', async () => {
  const rendered = await renderSheet();
  expect(
    screen.getByTestId('display-option-1').props.nativeBackgroundAndroid,
  ).toBeUndefined();
  expect(
    screen.getByTestId('quick-action-view-only').props.nativeBackgroundAndroid,
  ).toBeUndefined();

  await cleanup();
  rendered.queryClient.clear();
  jest.replaceProperty(Platform, 'OS', 'android');
  await renderSheet();

  const ripple = { type: 'RippleAndroid', borderless: false };
  expect(
    screen.getByTestId('display-option-1').props.nativeBackgroundAndroid,
  ).toMatchObject(ripple);
  expect(
    screen.getByTestId('quick-action-view-only').props.nativeBackgroundAndroid,
  ).toMatchObject(ripple);
});
