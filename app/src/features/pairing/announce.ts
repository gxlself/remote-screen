import type { PairUiState } from './machine';
import type { FailureModel } from './outcome';
import { pairWaiting } from '../../i18n/zh/pairWaiting';

export function pairStatusText(input: {
  phase: PairUiState['phase'];
  step: PairUiState['step'];
  failure: FailureModel | null;
  hostWord: string;
}): string | null {
  if (input.failure !== null) {
    return pairWaiting.a11y.status(input.failure.title, input.failure.message);
  }
  if (input.phase === 'progress' && input.step === 'waitingLocal') {
    return pairWaiting.a11y.status(
      pairWaiting.approveTitle(input.hostWord),
      pairWaiting.approveDetail(input.hostWord),
    );
  }
  return null;
}
