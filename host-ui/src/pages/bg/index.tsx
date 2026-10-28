import type { PageProps } from '../../app/router';

import { useOrchestrator } from './useOrchestrator';

export default function BackdropPage(_props: PageProps) {
  useOrchestrator();

  return <main data-page="bg" hidden />;
}
