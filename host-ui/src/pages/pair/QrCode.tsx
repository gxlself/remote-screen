import { useMemo } from 'react';
import * as QRCode from 'qrcode';

import { zh } from '../../i18n';

export interface QrCodeProps {
  value: string;
  size: number;
}

export function QrCode({ value, size }: QrCodeProps) {
  const modules = useMemo(
    () => QRCode.create(value, { errorCorrectionLevel: 'M' }).modules,
    [value],
  );
  const path = useMemo(() => {
    const commands: string[] = [];
    for (let row = 0; row < modules.size; row += 1) {
      for (let column = 0; column < modules.size; column += 1) {
        if (modules.get(row, column) === 1) {
          commands.push(`M${column} ${row}h1v1h-1z`);
        }
      }
    }
    return commands.join('');
  }, [modules]);

  return (
    <svg
      aria-label={zh.pair.qrAlt}
      height={size}
      role="img"
      shapeRendering="crispEdges"
      viewBox={`0 0 ${modules.size} ${modules.size}`}
      width={size}
    >
      <path d={path} fill="#000000" />
    </svg>
  );
}
