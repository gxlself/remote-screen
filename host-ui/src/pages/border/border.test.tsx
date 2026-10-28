import { screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import type { Route } from '../../app/router';
import { FakeIpc } from '../../ipc/fake';
import { renderPage } from '../../ipc/testing';

import BorderPage from '.';

const route: Route = { page: 'border', query: {} };

describe('BorderPage', () => {
  it('draws a non-interactive controlled border without IPC calls', () => {
    const ipc = new FakeIpc();
    renderPage(<BorderPage route={route} />, { ipc });

    const border = screen.getByTestId('controlled-border');
    expect(border).toHaveAttribute('aria-hidden', 'true');
    expect(ipc.calls).toEqual([]);
  });
});
