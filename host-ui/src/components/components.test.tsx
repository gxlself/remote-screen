import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';

import {
  AppGlyph,
  Banner,
  Button,
  Checkbox,
  CountdownRing,
  DeviceGlyph,
  Divider,
  KeyValueList,
  RadioGroup,
  ringOffset,
  StatusDot,
  Switch,
  Tag,
  WindowFrame,
} from '.';
import type { ButtonVariant, RadioGroupOption, TagTone } from '.';

describe('Button', () => {
  it('calls onClick and renders every variant as a native button', async () => {
    const user = userEvent.setup();
    const onClick = vi.fn();
    const variants: ButtonVariant[] = ['primary', 'secondary', 'danger', 'ghost'];
    const { rerender } = render(
      <Button onClick={onClick} variant="primary">
        Continue
      </Button>,
    );

    await user.click(screen.getByRole('button', { name: 'Continue' }));
    expect(onClick).toHaveBeenCalledTimes(1);

    for (const variant of variants) {
      rerender(<Button variant={variant}>Continue</Button>);
      expect(screen.getByRole('button', { name: 'Continue' })).toHaveAttribute('type', 'button');
    }
  });

  it('does not call onClick while disabled', async () => {
    const user = userEvent.setup();
    const onClick = vi.fn();
    render(
      <Button disabled onClick={onClick}>
        Disabled
      </Button>,
    );

    await user.click(screen.getByRole('button', { name: 'Disabled' }));
    expect(onClick).not.toHaveBeenCalled();
  });
});

describe('Switch', () => {
  it('reflects checked state, toggles on click and Space, and supports labels', async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    const { rerender } = render(
      <Switch checked={false} label="Notifications" onChange={onChange} />,
    );

    const off = screen.getByRole('switch', { name: 'Notifications' });
    expect(off).toHaveAttribute('aria-checked', 'false');
    expect(off).toHaveAttribute('aria-label', 'Notifications');

    await user.click(off);
    expect(onChange).toHaveBeenLastCalledWith(true);

    onChange.mockClear();
    await user.keyboard(' ');
    expect(onChange).toHaveBeenLastCalledWith(true);

    rerender(
      <>
        <span id="preference-label">Preference</span>
        <Switch checked labelledBy="preference-label" onChange={onChange} />
      </>,
    );
    const on = screen.getByRole('switch', { name: 'Preference' });
    expect(on).toHaveAttribute('aria-checked', 'true');
    expect(on).toHaveAttribute('aria-labelledby', 'preference-label');
    expect(on).not.toHaveAttribute('aria-label');
  });

  it('does not toggle while disabled', async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<Switch checked={false} disabled label="Notifications" onChange={onChange} />);

    await user.click(screen.getByRole('switch'));
    expect(onChange).not.toHaveBeenCalled();
  });
});

describe('RadioGroup', () => {
  const options: Array<RadioGroupOption<'a' | 'b' | 'c'>> = [
    { value: 'a', title: 'Automatic', description: 'Choose automatically.' },
    { value: 'b', title: 'Manual' },
    { value: 'c', title: 'Off' },
  ];

  it('reports the selected option and changes when another is clicked', async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<RadioGroup label="Mode" onChange={onChange} options={options} value="a" />);

    const radios = screen.getAllByRole('radio');
    expect(screen.getByRole('radiogroup', { name: 'Mode' })).toBeInTheDocument();
    expect(radios).toHaveLength(3);
    expect(radios[0]).toHaveAttribute('aria-checked', 'true');
    expect(radios[1]).toHaveAttribute('aria-checked', 'false');
    expect(screen.getByText('Choose automatically.')).toBeInTheDocument();

    await user.click(radios[1] as HTMLElement);
    expect(onChange).toHaveBeenCalledWith('b');
  });

  it('uses ArrowDown and skips disabled options', async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    const optionsWithDisabled: Array<RadioGroupOption<'a' | 'b' | 'c'>> = [
      { value: 'a', title: 'First' },
      { value: 'b', title: 'Second', disabled: true },
      { value: 'c', title: 'Third' },
    ];
    render(
      <RadioGroup label="Choice" onChange={onChange} options={optionsWithDisabled} value="a" />,
    );

    const first = screen.getByRole('radio', { name: 'First' });
    first.focus();
    await user.keyboard('{ArrowDown}');

    expect(onChange).toHaveBeenCalledWith('c');
    expect(screen.getByRole('radio', { name: 'Third' })).toHaveFocus();
  });

  it('supports aria-labelledby and the boxed layout', () => {
    render(
      <>
        <span id="group-label">Connection</span>
        <RadioGroup
          boxed
          labelledBy="group-label"
          onChange={() => undefined}
          options={options}
          value="a"
        />
      </>,
    );

    const group = screen.getByRole('radiogroup', { name: 'Connection' });
    expect(group).toHaveAttribute('aria-labelledby', 'group-label');
    expect(group.className).toContain('boxed');
  });
});

describe('Checkbox', () => {
  it('toggles and exposes the disabled state', async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    const { rerender } = render(
      <Checkbox checked={false} label="Remember this device" onChange={onChange} />,
    );

    const checkbox = screen.getByRole('checkbox', { name: 'Remember this device' });
    expect(checkbox).toHaveAttribute('aria-checked', 'false');
    await user.click(checkbox);
    expect(onChange).toHaveBeenCalledWith(true);

    onChange.mockClear();
    rerender(
      <Checkbox checked={false} disabled label="Remember this device" onChange={onChange} />,
    );
    const disabled = screen.getByRole('checkbox', { name: 'Remember this device' });
    expect(disabled).toHaveAttribute('aria-disabled', 'true');
    await user.click(disabled);
    expect(onChange).not.toHaveBeenCalled();
  });
});

describe('Tag', () => {
  it('renders children and gives each tone a distinct class', () => {
    const tones: TagTone[] = ['controlling', 'neutral', 'accent', 'relay', 'good', 'bad'];
    const { container } = render(
      <>
        {tones.map((tone) => (
          <Tag key={tone} tone={tone}>
            {tone}
          </Tag>
        ))}
      </>,
    );

    expect(screen.getByText('controlling')).toBeInTheDocument();
    const classes = Array.from(container.querySelectorAll('span')).map(
      (element) => element.className,
    );
    expect(new Set(classes).size).toBe(tones.length);
  });
});

describe('CountdownRing', () => {
  it('clamps progress and computes the stroke offset', () => {
    expect(ringOffset(30, 30, 40.8)).toBe(0);
    expect(ringOffset(30, 15, 40.8)).toBeCloseTo(20.4);
    expect(ringOffset(30, -5, 40.8)).toBe(40.8);
    expect(ringOffset(30, 99, 40.8)).toBe(0);
  });

  it('renders as a decorative SVG', () => {
    const { container } = render(<CountdownRing remaining={15} total={30} />);
    expect(container.querySelector('svg')).toHaveAttribute('aria-hidden', 'true');
  });
});

describe('DeviceGlyph', () => {
  it('distinguishes iPadOS from other platforms and is decorative', () => {
    const { container, rerender } = render(<DeviceGlyph platform="ipados" />);
    const tablet = container.firstElementChild;
    expect(tablet).toHaveAttribute('data-kind', 'tablet');
    expect(tablet).toHaveAttribute('aria-hidden', 'true');

    rerender(<DeviceGlyph platform="ios" />);
    const phone = container.firstElementChild;
    expect(phone).toHaveAttribute('data-kind', 'phone');
    expect(phone).toHaveAttribute('aria-hidden', 'true');
  });
});

describe('AppGlyph and StatusDot', () => {
  it('renders decorative glyphs', () => {
    const { container } = render(
      <>
        <AppGlyph />
        <StatusDot tone="good" />
      </>,
    );
    expect(container.querySelectorAll(':scope > [aria-hidden="true"]')).toHaveLength(2);
  });
});

describe('KeyValueList', () => {
  it('renders terms and descriptions while omitting null values', () => {
    const { container } = render(
      <KeyValueList
        items={[
          { key: 'Device', value: 'Mac mini' },
          { key: 'Hidden', value: null },
          { key: 'Address', value: '100.64.0.8', mono: true },
        ]}
      />,
    );

    expect(screen.getByText('Device')).toBeInTheDocument();
    expect(screen.getByText('Mac mini')).toBeInTheDocument();
    expect(screen.queryByText('Hidden')).not.toBeInTheDocument();
    expect(container.querySelectorAll('dt')).toHaveLength(2);
    expect(container.querySelectorAll('dd')).toHaveLength(2);
    expect(screen.getByText('100.64.0.8').className).toContain('mono');
  });
});

describe('WindowFrame', () => {
  it('connects the section to its heading and renders a dragged title bar and footer', () => {
    const { container } = render(
      <WindowFrame footer={<Button>Done</Button>} title="配对新设备" titleId="pair-title">
        <p>Content</p>
      </WindowFrame>,
    );

    expect(screen.getByRole('heading', { name: '配对新设备' })).toBeInTheDocument();
    expect(container.querySelector('[data-tauri-drag-region]')).toBeInTheDocument();
    expect(screen.getByText('Done')).toBeInTheDocument();
    expect(container.querySelector('section')).toHaveAttribute('aria-labelledby', 'pair-title');
  });
});

describe('Banner', () => {
  it('exposes an alert role when requested', () => {
    render(
      <Banner role="alert" title="Connection lost" tone="bad">
        Retrying
      </Banner>,
    );

    expect(screen.getByRole('alert')).toHaveTextContent('Connection lost');
  });
});

describe('Divider', () => {
  it('renders a horizontal rule', () => {
    render(<Divider />);
    expect(screen.getByRole('separator')).toBeInTheDocument();
  });
});
