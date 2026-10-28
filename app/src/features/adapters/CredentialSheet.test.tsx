import {
  fireEvent,
  screen,
  waitFor,
  within,
} from '@testing-library/react-native';
import { adapters } from '../../i18n/zh/adapters';
import type {
  AdapterCredField,
  AdapterProtocol,
} from '../../native/rscore/types';
import { renderWithProviders } from '../../test/render';
import { CredentialSheet } from './CredentialSheet';
import type { CredInput } from './logic';

type RenderSheetOptions = {
  protocol?: AdapterProtocol;
  fields?: readonly AdapterCredField[];
  failed?: boolean;
  canSave?: boolean;
  defaultUser?: string;
  defaultDomain?: string;
  /** What onSubmit reports back; true = handed to the adapter. */
  accepted?: boolean;
};

async function renderSheet(options: RenderSheetOptions = {}) {
  const onSubmit = jest.fn<boolean, [CredInput, boolean]>(
    () => options.accepted ?? true,
  );
  const onCancel = jest.fn<void, []>();
  const rendered = await renderWithProviders(
    <CredentialSheet
      canSave={options.canSave ?? true}
      defaultDomain={options.defaultDomain}
      defaultUser={options.defaultUser}
      failed={options.failed ?? false}
      fields={options.fields ?? ['password']}
      onCancel={onCancel}
      onSubmit={onSubmit}
      protocol={options.protocol ?? 'vnc'}
    />,
  );
  return { ...rendered, onCancel, onSubmit };
}

it('vnc_password_only shows only the password field', async () => {
  await renderSheet({ fields: ['password'] });

  expect(screen.queryByTestId('adapter-creds-user')).toBeNull();
  expect(screen.getByTestId('adapter-creds-password')).toBeOnTheScreen();
  expect(screen.getByText(adapters.creds.intro('vnc', true))).toBeOnTheScreen();
});

it('ard_fields shows user and password without a domain', async () => {
  await renderSheet({ fields: ['user', 'password'], protocol: 'vnc' });

  expect(screen.getByTestId('adapter-creds-user')).toBeOnTheScreen();
  expect(screen.getByTestId('adapter-creds-password')).toBeOnTheScreen();
  expect(screen.queryByTestId('adapter-creds-domain')).toBeNull();
  expect(
    screen.getByText(adapters.creds.intro('vnc', false)),
  ).toBeOnTheScreen();
});

it('rdp_fields shows user, password and domain in order', async () => {
  await renderSheet({
    fields: ['user', 'password', 'domain'],
    protocol: 'rdp',
  });

  expect(screen.getByText(adapters.creds.title('rdp'))).toBeOnTheScreen();
  const tree = JSON.stringify(screen.toJSON());
  expect(tree.indexOf('adapter-creds-user')).toBeLessThan(
    tree.indexOf('adapter-creds-password'),
  );
  expect(tree.indexOf('adapter-creds-password')).toBeLessThan(
    tree.indexOf('adapter-creds-domain'),
  );
});

it('empty_submit reports both required fields without submitting', async () => {
  const { onSubmit } = await renderSheet({ fields: ['user', 'password'] });

  await fireEvent.press(screen.getByText(adapters.creds.submit));

  expect(screen.getAllByText(adapters.creds.invalidEmpty)).toHaveLength(2);
  expect(onSubmit).not.toHaveBeenCalled();
});

it('too_long reports the password byte limit', async () => {
  const { onSubmit } = await renderSheet({ fields: ['password'] });

  await fireEvent.changeText(
    screen.getByTestId('adapter-creds-password'),
    'a'.repeat(1025),
  );
  await fireEvent.press(screen.getByText(adapters.creds.submit));

  expect(
    screen.getByText(adapters.creds.invalidLength(1024)),
  ).toBeOnTheScreen();
  expect(onSubmit).not.toHaveBeenCalled();
});

it('control_char rejects a control character in the user name', async () => {
  const { onSubmit } = await renderSheet({ fields: ['user', 'password'] });

  await fireEvent.changeText(
    screen.getByTestId('adapter-creds-user'),
    'a\u0007b',
  );
  await fireEvent.changeText(
    screen.getByTestId('adapter-creds-password'),
    'pw',
  );
  await fireEvent.press(screen.getByText(adapters.creds.submit));

  expect(screen.getByText(adapters.creds.invalidControl)).toBeOnTheScreen();
  expect(onSubmit).not.toHaveBeenCalled();
});

it('valid_submit sends the values and immediately clears the password', async () => {
  const { onSubmit } = await renderSheet({ fields: ['user', 'password'] });
  const password = screen.getByTestId('adapter-creds-password');
  expect(password).toHaveProp('secureTextEntry', true);
  expect(screen.getByTestId('adapter-creds-user')).toHaveProp(
    'autoCapitalize',
    'none',
  );
  expect(screen.getByTestId('adapter-creds-user')).toHaveProp(
    'autoCorrect',
    false,
  );

  await fireEvent.changeText(screen.getByTestId('adapter-creds-user'), 'alice');
  await fireEvent.changeText(
    screen.getByTestId('adapter-creds-password'),
    'pw',
  );
  await fireEvent.press(screen.getByText(adapters.creds.submit));

  expect(onSubmit).toHaveBeenCalledWith(
    { user: 'alice', password: 'pw', domain: '' },
    true,
  );
  expect(screen.getByTestId('adapter-creds-password').props.value).toBe('');
  expect(screen.getByTestId('adapter-creds-user').props.value).toBe('alice');

  await fireEvent.changeText(
    screen.getByTestId('adapter-creds-user'),
    'alice2',
  );
  expect(screen.getByTestId('adapter-creds-password').props.value).toBe('');
});

it('remember_off submits without saving', async () => {
  const { onSubmit } = await renderSheet({ fields: ['password'] });
  const remember = screen.getByRole('switch', {
    name: adapters.creds.remember,
  });
  expect(remember).toBeChecked();

  await fireEvent.press(remember);
  await fireEvent.changeText(
    screen.getByTestId('adapter-creds-password'),
    'pw',
  );
  await fireEvent.press(screen.getByText(adapters.creds.submit));

  expect(onSubmit).toHaveBeenCalledWith(
    { user: '', password: 'pw', domain: '' },
    false,
  );
});

it('cannot_save hides the remember switch and never saves', async () => {
  const { onSubmit } = await renderSheet({
    canSave: false,
    fields: ['password'],
  });

  expect(screen.queryByRole('switch')).toBeNull();
  expect(screen.queryByText(adapters.creds.remember)).toBeNull();
  await fireEvent.changeText(
    screen.getByTestId('adapter-creds-password'),
    'pw',
  );
  await fireEvent.press(screen.getByText(adapters.creds.submit));

  expect(onSubmit).toHaveBeenCalledWith(
    { user: '', password: 'pw', domain: '' },
    false,
  );
});

it('failed_banner shows the authentication failure', async () => {
  await renderSheet({ failed: true, fields: ['password'] });

  expect(screen.getByText(adapters.creds.failed(true))).toBeOnTheScreen();
  expect(screen.getByTestId('adapter-creds-failed')).toHaveProp(
    'accessibilityLiveRegion',
    'polite',
  );
});

it('prefill fills the saved user and domain', async () => {
  await renderSheet({
    defaultDomain: 'CORP',
    defaultUser: 'alice',
    fields: ['user', 'password', 'domain'],
    protocol: 'rdp',
  });

  expect(screen.getByTestId('adapter-creds-user').props.value).toBe('alice');
  expect(screen.getByTestId('adapter-creds-domain').props.value).toBe('CORP');
});

it('cancel calls onCancel once', async () => {
  const { onCancel } = await renderSheet();

  await fireEvent.press(screen.getByText(adapters.creds.cancel));

  expect(onCancel).toHaveBeenCalledTimes(1);
});

it('error_clears_on_edit removes only the edited field error', async () => {
  await renderSheet({ fields: ['user', 'password'] });
  await fireEvent.press(screen.getByText(adapters.creds.submit));
  expect(screen.getAllByText(adapters.creds.invalidEmpty)).toHaveLength(2);

  await fireEvent.changeText(screen.getByTestId('adapter-creds-user'), 'alice');

  await waitFor(() => {
    const field = screen.getByTestId('adapter-creds-user').parent;
    if (field == null) {
      throw new Error('user field container is missing');
    }
    expect(within(field).queryByText(adapters.creds.invalidEmpty)).toBeNull();
  });
  expect(screen.getAllByText(adapters.creds.invalidEmpty)).toHaveLength(1);
});

it('failed_banner names the user name only when it is asked for (V17)', async () => {
  await renderSheet({ failed: true, fields: ['user', 'password'] });

  expect(screen.getByText(adapters.creds.failed(false))).toBeOnTheScreen();
  expect(screen.queryByText(adapters.creds.failed(true))).toBeNull();
});

it('submitting shows loading and ignores a second tap (V16)', async () => {
  const { onSubmit } = await renderSheet({ fields: ['password'] });
  await fireEvent.changeText(
    screen.getByTestId('adapter-creds-password'),
    'pw',
  );
  await fireEvent.press(screen.getByTestId('adapter-creds-submit'));
  await fireEvent.press(screen.getByTestId('adapter-creds-submit'));

  expect(onSubmit).toHaveBeenCalledTimes(1);
  expect(screen.getByTestId('adapter-creds-submit')).toHaveProp(
    'accessibilityState',
    expect.objectContaining({ busy: true }),
  );
  // The cleared password must not turn the second tap into 必填.
  expect(screen.queryByText(adapters.creds.invalidEmpty)).toBeNull();
});

it('a submit that does not reach the computer says so and can be retried', async () => {
  const { onSubmit } = await renderSheet({
    accepted: false,
    fields: ['password'],
  });
  await fireEvent.changeText(
    screen.getByTestId('adapter-creds-password'),
    'pw',
  );
  await fireEvent.press(screen.getByTestId('adapter-creds-submit'));

  expect(screen.getByTestId('adapter-creds-submit-failed')).toBeOnTheScreen();
  expect(screen.getByText(adapters.creds.submitFailed)).toBeOnTheScreen();
  expect(screen.getByTestId('adapter-creds-submit')).not.toHaveProp(
    'accessibilityState',
    expect.objectContaining({ busy: true }),
  );

  await fireEvent.changeText(
    screen.getByTestId('adapter-creds-password'),
    'pw2',
  );
  await fireEvent.press(screen.getByTestId('adapter-creds-submit'));
  expect(onSubmit).toHaveBeenCalledTimes(2);
});

it('the whole remember row toggles remembering (V46)', async () => {
  const { onSubmit } = await renderSheet({ fields: ['password'] });

  await fireEvent.press(screen.getByTestId('adapter-creds-remember-row'));
  expect(
    screen.getByRole('switch', { name: adapters.creds.remember }),
  ).not.toBeChecked();
  await fireEvent.changeText(
    screen.getByTestId('adapter-creds-password'),
    'pw',
  );
  await fireEvent.press(screen.getByTestId('adapter-creds-submit'));
  expect(onSubmit).toHaveBeenCalledWith(
    { user: '', password: 'pw', domain: '' },
    false,
  );
});

it('fields autofill, chain with the return key and submit from the last (V15)', async () => {
  const { onSubmit } = await renderSheet({
    fields: ['user', 'password', 'domain'],
    protocol: 'rdp',
  });
  const user = screen.getByTestId('adapter-creds-user');
  const password = screen.getByTestId('adapter-creds-password');
  const domain = screen.getByTestId('adapter-creds-domain');
  expect(user).toHaveProp('textContentType', 'username');
  expect(user).toHaveProp('returnKeyType', 'next');
  expect(password).toHaveProp('textContentType', 'password');
  expect(password).toHaveProp('returnKeyType', 'next');
  expect(domain).toHaveProp('returnKeyType', 'go');
  expect(screen.getByTestId('adapter-creds-password-reveal')).toBeOnTheScreen();

  await fireEvent.changeText(user, 'alice');
  await fireEvent.changeText(password, 'pw');
  await fireEvent(password, 'submitEditing');
  expect(onSubmit).not.toHaveBeenCalled();
  await fireEvent(domain, 'submitEditing');
  expect(onSubmit).toHaveBeenCalledWith(
    { user: 'alice', password: 'pw', domain: '' },
    true,
  );
});

it('the password return key submits when there is no domain', async () => {
  const { onSubmit } = await renderSheet({ fields: ['password'] });
  const password = screen.getByTestId('adapter-creds-password');
  expect(password).toHaveProp('returnKeyType', 'go');

  await fireEvent.changeText(password, 'pw');
  await fireEvent(password, 'submitEditing');
  expect(onSubmit).toHaveBeenCalledTimes(1);
});

it('starts on the password when the user name is already known (V14)', async () => {
  await renderSheet({ defaultUser: 'alice', fields: ['user', 'password'] });

  expect(screen.getByTestId('adapter-creds-password')).toHaveProp(
    'autoFocus',
    true,
  );
  expect(screen.getByTestId('adapter-creds-user')).toHaveProp(
    'autoFocus',
    false,
  );
});

it('cannot be dismissed from the backdrop while submitting', async () => {
  const { onCancel } = await renderSheet({ fields: ['password'] });
  await fireEvent.changeText(
    screen.getByTestId('adapter-creds-password'),
    'pw',
  );
  await fireEvent.press(screen.getByTestId('adapter-creds-submit'));

  await fireEvent.press(screen.getByTestId('adapter-creds-backdrop'));
  expect(onCancel).not.toHaveBeenCalled();
  await fireEvent.press(screen.getByTestId('adapter-creds-cancel'));
  expect(onCancel).toHaveBeenCalledTimes(1);
});

it('can be dismissed from the backdrop before submitting', async () => {
  const { onCancel } = await renderSheet({ fields: ['password'] });

  await fireEvent.press(screen.getByTestId('adapter-creds-backdrop'));
  expect(onCancel).toHaveBeenCalledTimes(1);
});
