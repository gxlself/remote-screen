import { act, cleanup, render } from '@testing-library/react-native';
import { StyleSheet } from 'react-native';
import { videoStore } from '../../features/session/videoStore';
import FollowZoomChip from './FollowZoomChip';

afterEach(async () => {
  await cleanup();
});

beforeEach(() => {
  videoStore.setState({ stats: {}, viewport: {} });
});

it('shows the current zoom and is purely presentational', async () => {
  videoStore.setState({
    stats: {},
    viewport: { s1: { zoom: 1.7, x: 0.1, y: 0.1 } },
  });
  const view = await render(<FollowZoomChip sessionId="s1" />);
  expect(view.getByText('跟随输入位置')).toBeOnTheScreen();
  expect(view.getByText('1.7×')).toBeOnTheScreen();
  expect(view.getByTestId('follow-zoom-chip')).toHaveProp('accessible', false);
  expect(view.getByTestId('follow-zoom-chip')).toHaveStyle({
    height: 30,
    borderRadius: 15,
    backgroundColor: 'rgba(10,13,16,0.8)',
  });
  expect(StyleSheet.flatten(view.getByText('1.7×').props.style)).toMatchObject({
    color: '#A9B4BB',
    fontSize: 12,
  });
});

it('falls back to 1.0× without viewport data', async () => {
  const view = await render(<FollowZoomChip sessionId="s1" />);
  expect(view.getByText('1.0×')).toBeOnTheScreen();
  await act(() => {
    videoStore.setState({
      stats: {},
      viewport: { s1: { zoom: 2.4, x: 0.1, y: 0.1 } },
    });
  });
  expect(view.getByText('2.4×')).toBeOnTheScreen();
});
