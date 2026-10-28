import { region, visibleRect, zoomLabel } from './viewportText';

it('maps the normalized viewport to an in-bounds rect', () => {
  expect(visibleRect({ zoom: 2, x: 0.25, y: 0.25 })).toEqual({
    left: 0.25,
    top: 0.25,
    width: 0.5,
    height: 0.5,
  });
  expect(region(visibleRect({ zoom: 2, x: 0.25, y: 0.25 }))).toBe('中间');
  expect(region(visibleRect({ zoom: 3, x: 0, y: 0 }))).toBe('左上');
});

it('clamps zoomed-out and out-of-bounds viewports to the full screen', () => {
  expect(visibleRect({ zoom: 0.5, x: 0.4, y: 0.7 })).toEqual({
    left: 0,
    top: 0,
    width: 1,
    height: 1,
  });
  expect(visibleRect({ zoom: 2, x: 0.9, y: -0.3 })).toEqual({
    left: 0.5,
    top: 0,
    width: 0.5,
    height: 0.5,
  });
});

it('classifies each third of the screen', () => {
  const oneThird = 1 / 3;
  const rectAt = (left: number, top: number) => ({
    left,
    top,
    width: oneThird,
    height: oneThird,
  });
  expect(region(rectAt(0, 0))).toBe('左上');
  expect(region(rectAt(oneThird, 0))).toBe('上方');
  expect(region(rectAt(2 * oneThird, 0))).toBe('右上');
  expect(region(rectAt(0, oneThird))).toBe('左侧');
  expect(region(rectAt(oneThird, oneThird))).toBe('中间');
  expect(region(rectAt(2 * oneThird, oneThird))).toBe('右侧');
  expect(region(rectAt(0, 2 * oneThird))).toBe('左下');
  expect(region(rectAt(oneThird, 2 * oneThird))).toBe('下方');
  expect(region(rectAt(2 * oneThird, 2 * oneThird))).toBe('右下');
  expect(region({ left: 1 / 3, top: 0, width: 0, height: 0 })).toBe('上方');
  expect(region({ left: 2 / 3, top: 0, width: 0, height: 0 })).toBe('右上');
});

it('formats zoom to one decimal with a multiplication sign', () => {
  expect(zoomLabel(1.666)).toBe('1.7×');
  expect(zoomLabel(1)).toBe('1.0×');
  expect(zoomLabel(Number.NaN)).toBe('1.0×');
});
