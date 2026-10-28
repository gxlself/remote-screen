import type { JSX } from 'react';
import { Modal } from 'react-native';
import { BottomSheet } from '../../components/BottomSheet';
import { Icon } from '../../components/icons/Icon';
import { ListGroup, ListRow } from '../../components/List';
import { adapters } from '../../i18n/zh/adapters';
import type { AdapterScreen } from '../../native/rscore/types';
import { formatResolution } from '../format/latency';
import { MonitorCountPicker } from './MonitorCountPicker';
import type { MonitorCount } from './monitorMemory';

/** adapters.selectScreen 的下标：整个桌面（Rust `session::WHOLE_DESKTOP`）。 */
export const WHOLE_DESKTOP = 255;

export type ScreenPickerProps = {
  screens: AdapterScreen[];
  /** 当前显示的那台（screens 的下标，0 = 主显示器），或 WHOLE_DESKTOP。 */
  selected: number;
  onSelect(index: number): void;
  onClose(): void;
  /** 远程桌面才有：显示器数量。 */
  monitors?: {
    count: MonitorCount;
    onChange(count: MonitorCount): void;
  };
};

/** 推断出来的屏（没有主屏信息）按位置命名：左右并排或上下排列。 */
function guessedName(screens: AdapterScreen[], index: number): string {
  const names = adapters.screens.guessed;
  const [first, second] = screens;
  const sideBySide =
    first !== undefined &&
    second !== undefined &&
    second.x >= first.x + first.width;
  if (index === 0) {
    return sideBySide ? names.left : names.top;
  }
  if (index === screens.length - 1) {
    return sideBySide ? names.right : names.bottom;
  }
  if (screens.length === 3) {
    return names.middle;
  }
  return sideBySide ? names.fromLeft(index + 1) : names.fromTop(index + 1);
}

/** 所有显示器拼成的桌面尺寸。 */
function desktopSize(screens: AdapterScreen[]): string {
  const left = Math.min(...screens.map(s => s.x));
  const top = Math.min(...screens.map(s => s.y));
  const right = Math.max(...screens.map(s => s.x + s.width));
  const bottom = Math.max(...screens.map(s => s.y + s.height));
  return formatResolution(right - left, bottom - top);
}

function Check({ testID }: { testID: string }): JSX.Element {
  return <Icon color="accent" name="check" size={18} testID={testID} />;
}

export function ScreenPicker(p: ScreenPickerProps): JSX.Element {
  const allSelected = p.selected === WHOLE_DESKTOP;
  const guessed = p.screens.some(screen => screen.guess !== undefined);
  const byAspect = p.screens.some(screen => screen.guess === 'aspect');
  const list = p.screens.length > 1;
  return (
    <Modal animationType="fade" onRequestClose={p.onClose} transparent visible>
      <BottomSheet
        onClose={p.onClose}
        subtitle={byAspect ? adapters.screens.aspectNote : undefined}
        testID="adapter-screens"
        title={list ? adapters.screens.title : adapters.toolbar.screens}
      >
        {list ? (
          <ListGroup>
            <ListRow
              accessibilityState={{ selected: allSelected }}
              onPress={() => p.onSelect(WHOLE_DESKTOP)}
              right={
                allSelected ? (
                  <Check testID="adapter-screen-all-check" />
                ) : undefined
              }
              testID="adapter-screen-all"
              title={adapters.screens.all}
              value={desktopSize(p.screens)}
            />
            {p.screens.map((screen, index) => {
              const selected = index === p.selected;
              return (
                <ListRow
                  accessibilityState={{ selected }}
                  key={index}
                  onPress={() => p.onSelect(index)}
                  right={
                    selected ? (
                      <Check testID={`adapter-screen-${index}-check`} />
                    ) : undefined
                  }
                  subtitle={
                    index === 0 && !guessed
                      ? adapters.screens.primary
                      : undefined
                  }
                  testID={`adapter-screen-${index}`}
                  title={
                    guessed
                      ? guessedName(p.screens, index)
                      : adapters.screens.item(index + 1)
                  }
                  value={formatResolution(screen.width, screen.height)}
                />
              );
            })}
          </ListGroup>
        ) : null}
        {p.monitors === undefined ? null : (
          <MonitorCountPicker
            count={p.monitors.count}
            onChange={p.monitors.onChange}
          />
        )}
      </BottomSheet>
    </Modal>
  );
}

export default ScreenPicker;
