import styles from './AppGlyph.module.css';

export interface AppGlyphProps {
  size?: number;
}

export function AppGlyph({ size = 32 }: AppGlyphProps) {
  const glyphSize = Math.min(24, size);

  return (
    <span
      aria-hidden="true"
      className={styles.glyph}
      style={{ width: size, height: size, borderRadius: size / 4 }}
    >
      <svg
        aria-hidden="true"
        fill="none"
        height={glyphSize}
        stroke="currentColor"
        strokeLinecap="round"
        strokeLinejoin="round"
        strokeWidth="1.5"
        viewBox="0 0 24 24"
        width={glyphSize}
      >
        <rect height="12.5" rx="2" width="18" x="3" y="4" />
        <path d="M9 20h6M12 16.5V20" />
      </svg>
    </span>
  );
}
