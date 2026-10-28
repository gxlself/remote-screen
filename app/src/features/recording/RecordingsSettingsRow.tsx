import type { ReactElement } from 'react';
import type { TabScreenProps } from '../../app/navigation/routes';
import { ListRow } from '../../components/List';
import { recordings } from '../../i18n/zh/recordings';
import { useCore } from '../../native/rscore/CoreContext';

export function useRecordingsSettingsRow(
  navigation: TabScreenProps<'Settings'>['navigation'],
): ReactElement | null {
  const core = useCore();
  if (core.recording === undefined) {
    return null;
  }

  return (
    <ListRow
      chevron
      onPress={() => navigation.navigate('Recordings')}
      subtitle={recordings.settingsRowSubtitle}
      testID="settings-recordings"
      title={recordings.settingsRow}
    />
  );
}
