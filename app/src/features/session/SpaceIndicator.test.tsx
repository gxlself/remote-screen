import { screen } from '@testing-library/react-native';
import { renderWithProviders } from '../../test/render';
import { SpaceIndicator } from './SpaceIndicator';

const spaces = [
  { id: 'a', name: 'Office Mac' },
  { id: 'b', name: 'Windows 台式机' },
  { id: 'c', name: 'Studio' },
];

it('marks the computer in front among one dot per computer', async () => {
  await renderWithProviders(
    <SpaceIndicator
      fading={false}
      highlightId="b"
      reduceMotion={false}
      spaces={spaces}
    />,
  );

  expect(screen.getByTestId('space-indicator-name')).toHaveTextContent(
    'Windows 台式机',
  );
  expect(screen.getByTestId('space-dot-0')).not.toBeSelected();
  expect(screen.getByTestId('space-dot-1')).toBeSelected();
  expect(screen.getByTestId('space-dot-2')).not.toBeSelected();
  expect(screen.getByTestId('space-indicator')).toHaveProp(
    'accessibilityLabel',
    '第 2 台，共 3 台：Windows 台式机',
  );
  // Never in the way of the fingers.
  expect(screen.getByTestId('space-indicator-layer')).toHaveProp(
    'pointerEvents',
    'none',
  );
});

it('draws nothing for a computer that is not in the row', async () => {
  await renderWithProviders(
    <SpaceIndicator
      fading={false}
      highlightId="gone"
      reduceMotion={false}
      spaces={spaces}
    />,
  );
  expect(screen.queryByTestId('space-indicator')).toBeNull();
});
