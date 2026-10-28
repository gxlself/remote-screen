import { useState } from 'react';

import { AppGlyph, Banner, Button, Checkbox, Divider, KeyValueList } from '../../../components';
import { zh } from '../../../i18n';
import { useIpc, useIpcQuery } from '../../../ipc';
import { usePlatform } from '../../../platform';

import styles from '../Settings.module.css';

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export default function AboutSection() {
  const ipc = useIpc();
  const platform = usePlatform();
  const status = useIpcQuery('status.get');

  const [mask, setMask] = useState(true);
  const [exporting, setExporting] = useState(false);
  const [exportedPath, setExportedPath] = useState<string | null>(null);
  const [exportError, setExportError] = useState<string | null>(null);

  const copy = zh.settings.about;
  const addresses = status.data?.tailnet_addresses ?? [];
  const tailnet = addresses.length > 0 ? addresses.join(', ') : copy.notConnected;

  const exportLogs = async (): Promise<void> => {
    setExporting(true);
    setExportedPath(null);
    setExportError(null);

    try {
      const result = await ipc.call('logs.export', { mask });
      setExportedPath(result.path);
    } catch (error) {
      setExportError(errorMessage(error));
    } finally {
      setExporting(false);
    }
  };

  return (
    <section className={styles.section} data-section="about">
      <h2 className={styles.sectionTitle}>{copy.title}</h2>

      <div className={styles.aboutHero}>
        <AppGlyph size={56} />
        <div>
          <div className={styles.aboutName}>{copy.name}</div>
          <div className={styles.aboutVersion}>{status.data?.version ?? ''}</div>
        </div>
      </div>

      <div className={styles.aboutDetails}>
        <KeyValueList items={[{ key: copy.tailnetAddress, value: tailnet, mono: true }]} />
      </div>

      <div className={styles.aboutActions}>
        <Button onClick={() => void platform.showWindow('update', 'update')} variant="secondary">
          {copy.softwareUpdate}
        </Button>
      </div>

      <Divider />

      <div className={styles.exportBlock}>
        <Checkbox
          checked={mask}
          disabled={exporting}
          label={copy.hideSensitive}
          onChange={setMask}
        />
        <Button disabled={exporting} onClick={() => void exportLogs()} variant="secondary">
          {exporting ? copy.exporting : copy.exportLogs}
        </Button>

        {exportError === null ? null : (
          <Banner role="alert" tone="bad">{`${copy.exportFailed}${exportError}`}</Banner>
        )}

        {exportedPath === null ? null : (
          <>
            <p className={styles.exportResult}>{`${copy.exportedTo} ${exportedPath}`}</p>
            <Button onClick={() => void platform.revealPath(exportedPath)} variant="ghost">
              {copy.revealInFinder}
            </Button>
          </>
        )}
      </div>
    </section>
  );
}
