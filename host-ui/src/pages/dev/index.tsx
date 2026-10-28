import { ROUTES } from '../../app/routes';
import { SETTINGS_SECTIONS } from '../settings/sections';
import { zh } from '../../i18n';

import type { PageProps } from '../../app/router';

import styles from './page.module.css';

export default function DevPage(_props: PageProps) {
  return (
    <main data-page="dev" className={styles.page}>
      <h1 className={styles.title}>
        {zh.common.appName} · {zh.common.dev.title}
      </h1>
      <p className={styles.lead}>{zh.common.dev.intro}</p>

      <section className={styles.section}>
        <h2 className={styles.heading}>{zh.common.dev.pages}</h2>
        <ul className={styles.list}>
          {Object.entries(ROUTES).map(([page, entry]) => (
            <li key={page}>
              <a className={styles.link} href={`#/${page}`}>
                <span>{entry.title}</span>
                <span className={styles.route}>#/{page}</span>
              </a>
            </li>
          ))}
        </ul>
      </section>

      <section className={styles.section}>
        <h2 className={styles.heading}>{zh.common.dev.settingsSections}</h2>
        <ul className={styles.list}>
          {SETTINGS_SECTIONS.map((section) => (
            <li key={section.id}>
              <a className={styles.link} href={`#/settings/${section.id}`}>
                <span>{zh.common.settingsNav[section.id]}</span>
                <span className={styles.route}>#/settings/{section.id}</span>
              </a>
            </li>
          ))}
        </ul>
      </section>

      <p className={styles.note}>{zh.common.dev.scenarioHint}</p>
    </main>
  );
}
