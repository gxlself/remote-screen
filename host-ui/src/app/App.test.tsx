import { act, render } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { App } from './App';

describe('App', () => {
  it('renders the menu route and updates document metadata', () => {
    location.hash = '#/menu';
    render(<App />);

    expect(document.querySelector('[data-page="menu"]')).not.toBeNull();
    expect(document.title).toBe('Remote Host · 菜单');
    expect(document.documentElement.dataset.page).toBe('menu');
  });

  it('reacts to hash changes', () => {
    location.hash = '#/menu';
    render(<App />);
    expect(document.querySelector('[data-page="menu"]')).not.toBeNull();

    act(() => {
      location.hash = '#/pair';
      window.dispatchEvent(new HashChangeEvent('hashchange'));
    });

    expect(document.querySelector('[data-page="pair"]')).not.toBeNull();
    expect(document.title).toBe('Remote Host · 配对新设备');
  });
});
