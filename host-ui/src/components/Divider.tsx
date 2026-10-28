import styles from './Divider.module.css';

export type DividerProps = Record<string, never>;

export function Divider() {
  return <hr className={styles.divider} />;
}
