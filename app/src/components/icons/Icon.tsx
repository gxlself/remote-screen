import type { JSX } from 'react';
import Svg, { Circle, Ellipse, Path, Rect } from 'react-native-svg';
import { tokens, type ThemeTokens } from '../../theme/tokens';
import { iconShapes, type IconName, type Shape } from './shapes';

export type { IconName };

type Props = {
  name: IconName;
  size?: number;
  color?: keyof ThemeTokens | string;
  strokeWidth?: number;
  accessibilityLabel?: string;
  testID?: string;
};

function resolveColor(color: keyof ThemeTokens | string): string {
  return Object.prototype.hasOwnProperty.call(tokens, color)
    ? tokens[color as keyof ThemeTokens]
    : color;
}

function renderShape(shape: Shape, key: string, color: string): JSX.Element {
  switch (shape.t) {
    case 'path':
      return <Path key={key} d={shape.d} />;
    case 'rect':
      return (
        <Rect
          key={key}
          x={shape.x}
          y={shape.y}
          width={shape.w}
          height={shape.h}
          rx={shape.rx}
        />
      );
    case 'circle':
      return (
        <Circle
          key={key}
          cx={shape.cx}
          cy={shape.cy}
          r={shape.r}
          fill={shape.filled ? color : undefined}
          stroke={shape.filled ? 'none' : undefined}
        />
      );
    case 'ellipse':
      return (
        <Ellipse
          key={key}
          cx={shape.cx}
          cy={shape.cy}
          rx={shape.rx}
          ry={shape.ry}
        />
      );
  }
}

export function Icon({
  name,
  size = 20,
  color = 'text',
  strokeWidth = 1.8,
  accessibilityLabel,
  testID,
}: Props): JSX.Element {
  const resolvedColor = resolveColor(color);
  const accessibilityProps = accessibilityLabel
    ? {
        accessible: true,
        accessibilityRole: 'image' as const,
        accessibilityLabel,
      }
    : {
        accessible: false,
        importantForAccessibility: 'no-hide-descendants' as const,
        accessibilityElementsHidden: true,
      };

  return (
    <Svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke={resolvedColor}
      strokeWidth={strokeWidth}
      strokeLinecap="round"
      strokeLinejoin="round"
      testID={testID}
      {...accessibilityProps}
    >
      {iconShapes[name].map((shape, index) =>
        renderShape(shape, `${name}-${index}`, resolvedColor),
      )}
    </Svg>
  );
}
