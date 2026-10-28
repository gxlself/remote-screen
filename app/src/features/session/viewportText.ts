import { sessionViewport } from '../../i18n/zh/sessionViewport';
import type { VideoViewport } from './videoStore';

export type VisibleRect = {
  left: number;
  top: number;
  width: number;
  height: number;
};

export type ViewportRegion =
  (typeof sessionViewport.regions)[keyof typeof sessionViewport.regions];

const REGION_GRID = [
  [
    sessionViewport.regions.topLeft,
    sessionViewport.regions.top,
    sessionViewport.regions.topRight,
  ],
  [
    sessionViewport.regions.left,
    sessionViewport.regions.middle,
    sessionViewport.regions.right,
  ],
  [
    sessionViewport.regions.bottomLeft,
    sessionViewport.regions.bottom,
    sessionViewport.regions.bottomRight,
  ],
] as const;

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

function finiteOr(value: number, fallback: number): number {
  return Number.isFinite(value) ? value : fallback;
}

function third(value: number): 0 | 1 | 2 {
  const index = Math.floor(clamp(value, 0, 1) * 3);
  return Math.min(2, index) as 0 | 1 | 2;
}

export function visibleRect(viewport: VideoViewport): VisibleRect {
  const zoom =
    Number.isFinite(viewport.zoom) && viewport.zoom > 0 ? viewport.zoom : 1;
  const width = clamp(1 / zoom, 0, 1);
  const height = width;
  const left = clamp(finiteOr(viewport.x, 0), 0, 1 - width);
  const top = clamp(finiteOr(viewport.y, 0), 0, 1 - height);
  return { left, top, width, height };
}

export function region(rect: VisibleRect): ViewportRegion {
  const column = third(rect.left + rect.width / 2);
  const row = third(rect.top + rect.height / 2);
  return REGION_GRID[row][column];
}

export function zoomLabel(zoom: number): string {
  const value = Number.isFinite(zoom) && zoom > 0 ? zoom : 1;
  return `${value.toFixed(1)}×`;
}
