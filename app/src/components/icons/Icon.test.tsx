import { render, screen } from '@testing-library/react-native';
import { processColor } from 'react-native';
import { tokens } from '../../theme/tokens';
import { Icon } from './Icon';
import { iconShapes } from './shapes';

function iconNode(testID = 'icon') {
  return screen.getByTestId(testID, { includeHiddenElements: true });
}

function svgProps() {
  return iconNode().props;
}

function nodesOfType(type: string) {
  return iconNode().queryAll(node => node.type === type);
}

it('contains exactly the 73 canvas icons', () => {
  expect(Object.keys(iconShapes)).toHaveLength(76);
});

it.each(Object.keys(iconShapes))('renders %s without throwing', async name => {
  await render(<Icon name={name as keyof typeof iconShapes} testID="icon" />);
  expect(iconNode()).toBeOnTheScreen();
});

it('renders check as one path with the canvas geometry', async () => {
  await render(<Icon name="check" testID="icon" />);
  const paths = nodesOfType('RNSVGPath');
  expect(paths).toHaveLength(1);
  expect(paths[0].props.d).toBe('M5 12.5l4.5 4.5L19 7.5');
});

it('renders computer as a rect and a path', async () => {
  await render(<Icon name="computer" testID="icon" />);
  const rects = nodesOfType('RNSVGRect');
  expect(rects).toHaveLength(1);
  expect(rects[0].props).toMatchObject({
    x: 3,
    y: 4,
    width: 18,
    height: 12.5,
    rx: 2,
  });
  expect(nodesOfType('RNSVGPath')).toHaveLength(1);
});

it('renders more as three circles', async () => {
  await render(<Icon name="more" testID="icon" />);
  expect(nodesOfType('RNSVGCircle')).toHaveLength(3);
});

it('uses size 20 by default and accepts 24', async () => {
  await render(<Icon name="check" testID="icon" />);
  expect(svgProps().style).toEqual(
    expect.arrayContaining([
      expect.objectContaining({ width: 20, height: 20 }),
    ]),
  );
  expect(svgProps().bbWidth).toBe(20);
  expect(svgProps().bbHeight).toBe(20);

  await render(<Icon name="check" size={24} testID="icon" />);
  expect(svgProps().bbWidth).toBe(24);
  expect(svgProps().bbHeight).toBe(24);
});

it.each([
  [undefined, tokens.text],
  ['accent' as const, '#7DA8FF'],
  ['#123456', '#123456'],
])('resolves color %s to %s', async (color, expected) => {
  await render(<Icon name="check" color={color} testID="icon" />);
  expect(nodesOfType('RNSVGGroup')[0].props.stroke).toEqual({
    type: 0,
    payload: processColor(expected),
  });
});

it('keeps an unlabeled icon out of the accessibility tree', async () => {
  await render(<Icon name="close" testID="icon" />);
  expect(svgProps().importantForAccessibility).toBe('no-hide-descendants');
  expect(svgProps().accessible).toBe(false);
  expect(svgProps().accessibilityElementsHidden).toBe(true);
  expect(screen.queryByLabelText('关闭')).toBeNull();
});

it('exposes a labelled icon as an image', async () => {
  await render(<Icon name="close" accessibilityLabel="关闭" testID="icon" />);
  expect(screen.getByLabelText('关闭')).toHaveProp(
    'accessibilityRole',
    'image',
  );
  expect(screen.getByLabelText('关闭')).toHaveProp('accessible', true);
});

it('renders the wifi dot as a filled circle without a stroke', async () => {
  await render(<Icon name="wifi" color="#123456" testID="icon" />);
  const dot = nodesOfType('RNSVGCircle')[0];
  expect(dot.props.fill).toEqual({
    type: 0,
    payload: processColor('#123456'),
  });
  expect(dot.props.stroke).toBeNull();
});

it('renders each supported primitive type', async () => {
  await render(<Icon name="database" testID="icon" />);
  expect(nodesOfType('RNSVGEllipse')).toHaveLength(1);
});
