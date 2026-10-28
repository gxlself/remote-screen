import type { ReactElement } from 'react';
import { Platform } from 'react-native';
import { ListRow } from '../../components/List';
import { Icon } from '../../components/icons/Icon';
import { pip } from '../../i18n/zh/pip';
import type { SessionCtx } from '../../screens/Session/slots';
import { useVideoStats } from '../session/videoStore';
import { pipEligibility } from './eligibility';
import { pipSupported, startPip } from './nativePip';
import { usePipState } from './pipState';

/** 「更多」面板里的「画中画」行；不显示时返回 null。 */
export function usePipMoreRow(
  ctx: SessionCtx,
  onClose: () => void,
): ReactElement | null {
  const stats = useVideoStats(ctx.sessionId);
  const pipState = usePipState();
  const eligibility = pipEligibility({
    role: ctx.view.role,
    overlay: ctx.view.overlay,
    phase: ctx.rec?.phase,
    width: stats?.width ?? 0,
    height: stats?.height ?? 0,
  });

  if (Platform.OS !== 'ios' || !pipSupported() || pipState.active) {
    return null;
  }
  if (ctx.view.role === 'controller') {
    // 控制者也看得到这一行，说明为什么用不了，而不是整行消失。
    return (
      <ListRow
        disabled
        left={<Icon color="text3" name="displays" size={20} />}
        subtitle={pip.controllerSubtitle}
        testID="more-pip-unavailable"
        title={pip.moreRow}
      />
    );
  }
  if (!eligibility.canStart) {
    return null;
  }

  return (
    <ListRow
      left={<Icon color="text2" name="displays" size={20} />}
      onPress={() => {
        onClose();
        startPip();
      }}
      subtitle={pip.moreRowSubtitle}
      testID="more-pip"
      title={pip.moreRow}
    />
  );
}
