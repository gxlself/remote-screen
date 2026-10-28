import { useEffect, useState, type JSX } from 'react';

import { toRsCoreError } from '@app/native/rscore/errors';
import { Banner, Button, DeviceGlyph, Tag } from '@host-ui/components';

import { useDesktop } from '../../core/CoreContext';

import styles from './computers.module.css';
import { glyphPlatform, sessionOs, type ComputerRow } from './model';
import { text } from './text';
import { useComputers } from './useComputers';

export type ComputersPageProps = {
  /** Add a computer by address. */
  onPair: () => void;
  /** Every list answer, so session notices can name the computer. */
  onRows: (rows: ComputerRow[]) => void;
};

export function ComputersPage({ onPair, onRows }: ComputersPageProps): JSX.Element {
  const desktop = useDesktop();
  const rows = useComputers();
  const [opening, setOpening] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);

  useEffect(() => {
    if (rows !== null) {
      onRows(rows);
    }
  }, [rows, onRows]);

  const open = async (row: ComputerRow, control: boolean) => {
    setOpening(true);
    setNotice(null);
    try {
      const settings = await desktop.settings().catch(() => null);
      await desktop.openSession({
        hostId: row.hostId,
        hostName: row.name,
        hostOs: sessionOs(row.os),
        control,
        ...(settings === null ? {} : { quality: settings.defaultQuality }),
      });
    } catch (e) {
      const error = toRsCoreError(e);
      setNotice(error.kind === 'unsupported' ? text.noDecoder : error.message);
    } finally {
      setOpening(false);
    }
  };

  return (
    <section className={styles.page}>
      <header className={styles.header}>
        <h1 className={styles.title}>{text.title}</h1>
        <div className={styles.headerActions}>
          <Button onClick={onPair} size="sm" variant="secondary">
            {text.addByAddress}
          </Button>
        </div>
      </header>

      {notice !== null ? <Banner role="alert" title={notice} tone="bad" /> : null}

      {rows === null ? (
        <p className={styles.hint} role="status">
          {text.loading}
        </p>
      ) : rows.length === 0 ? (
        <p className={styles.hint}>{text.empty}</p>
      ) : (
        <ul aria-label={text.listLabel} className={styles.list}>
          {rows.map((row) => (
            <li className={styles.row} key={row.hostId}>
              <DeviceGlyph platform={glyphPlatform(row.os)} />
              <div className={styles.info}>
                <div className={styles.name}>{row.name}</div>
                <div className={styles.meta}>
                  <Tag tone="accent">{text.paired}</Tag>
                </div>
              </div>
              <div className={styles.actions}>
                <Button disabled={opening} onClick={() => void open(row, true)} size="sm">
                  {text.connect}
                </Button>
                <Button
                  disabled={opening}
                  onClick={() => void open(row, false)}
                  size="sm"
                  variant="secondary"
                >
                  {text.viewOnly}
                </Button>
              </div>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
