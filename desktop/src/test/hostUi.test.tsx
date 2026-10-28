import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { Banner, Button } from '@host-ui/components';

describe('host-ui reuse', () => {
  it('host_ui_component_renders', () => {
    render(
      <Banner title="标题" tone="bad">
        <Button>继续</Button>
      </Banner>,
    );

    const button = screen.getByRole('button', { name: '继续' });
    expect(button).toBeInTheDocument();
    // CSS Modules from ../host-ui resolve to class names.
    expect(button.className).toMatch(/button/);
    expect(screen.getByText('标题')).toBeInTheDocument();
  });
});
