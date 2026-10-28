import { render } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { DeviceGlyph } from './DeviceGlyph';

describe('DeviceGlyph', () => {
  it('uses a computer glyph for desktop platforms', () => {
    const { container } = render(<DeviceGlyph platform="macos" />);
    expect(container.firstElementChild).toHaveAttribute('data-kind', 'computer');
  });
});
