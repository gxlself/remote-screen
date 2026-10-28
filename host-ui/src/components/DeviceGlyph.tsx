import styles from './DeviceGlyph.module.css';

export interface DeviceGlyphProps {
  platform: string;
  size?: number;
}

export function DeviceGlyph({ platform, size = 30 }: DeviceGlyphProps) {
  const p = platform.toLowerCase();
  const kind = p === 'ipados' ? 'tablet' : p === 'macos' || p === 'windows' ? 'computer' : 'phone';
  const glyphSize = Math.min(24, size);

  return (
    <span
      aria-hidden="true"
      className={styles.tile}
      data-kind={kind}
      style={{ width: size, height: size }}
    >
      <svg
        aria-hidden="true"
        fill="none"
        height={glyphSize}
        stroke="currentColor"
        strokeLinecap="round"
        strokeWidth="1.5"
        viewBox="0 0 24 24"
        width={glyphSize}
      >
        {kind === 'tablet' ? (
          <rect height="19" rx="2.5" width="16" x="4" y="2.5" />
        ) : kind === 'computer' ? (
          <>
            <rect height="12" rx="2" width="18" x="3" y="4" />
            <path d="M9 20h6M12 16v4" />
          </>
        ) : (
          <>
            <rect height="19" rx="2.5" width="10" x="7" y="2.5" />
            <path d="M10.5 18.5h3" />
          </>
        )}
      </svg>
    </span>
  );
}
