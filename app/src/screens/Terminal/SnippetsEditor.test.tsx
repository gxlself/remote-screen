import {
  act,
  cleanup,
  fireEvent,
  screen,
  waitFor,
} from '@testing-library/react-native';
import type { QueryClient } from '@tanstack/react-query';
import { createFakeCore } from '../../native/rscore/fakeCore';
import { LOCAL_SYNC_PREFIX, localSyncGet } from '../../features/data/localSync';
import { MemoryKv } from '../../features/storage/kv';
import type { KvStore } from '../../features/storage/kv';
import type { Snippet, SnippetsValue } from '../../features/terminal/snippets';
import { SNIPPET_CMD_MAX, SNIPPET_MAX } from '../../features/terminal/snippets';
import { common } from '../../i18n/zh/common';
import { failLocalSync, seedLocalSync } from '../../test/localDirectory';
import { renderWithProviders } from '../../test/render';
import SnippetsEditor from './SnippetsEditor';

const initial: Snippet[] = [
  { id: 'docker', label: '', cmd: 'docker ps', run: true },
  { id: 'tail', label: '', cmd: 'tail -f api.log', run: true },
  { id: 'systemctl', label: '', cmd: 'systemctl status api', run: true },
];

const clients: QueryClient[] = [];
// Dialogs sit beside the editor's modal, which hides its siblings.
const HIDDEN = { includeHiddenElements: true };

jest.setTimeout(30000);

function initialValue(items: Snippet[] = initial): SnippetsValue {
  return { v: 1, items };
}

/** Renders with `items` saved on this phone at version 1. */
async function renderEditor(
  items: Snippet[] = initial,
  setup?: (kv: KvStore) => void,
) {
  const onClose = jest.fn();
  const kv = new MemoryKv();
  seedLocalSync(kv, 'snippets', { version: 1, value: initialValue(items) });
  setup?.(kv);
  const rendered = await renderWithProviders(
    <SnippetsEditor onClose={onClose} />,
    { core: createFakeCore(), kv },
  );
  clients.push(rendered.queryClient);
  await waitFor(() => {
    expect(screen.queryByTestId('snippets-loading')).toBeNull();
  });
  return { ...rendered, onClose };
}

async function press(label: string) {
  await fireEvent.press(screen.getByLabelText(label));
  await act(async () => {
    await Promise.resolve();
  });
}

function snippetRows() {
  return screen.getAllByLabelText(/^编辑片段 /);
}

function saved(kv: KvStore) {
  return localSyncGet<SnippetsValue>(kv, 'snippets');
}

afterEach(async () => {
  await cleanup();
  for (const client of clients.splice(0)) {
    client.clear();
  }
  jest.restoreAllMocks();
});

it('renders the synced snippets with their run mode', async () => {
  await renderEditor();

  expect(screen.getByRole('header')).toHaveTextContent('命令片段');
  expect(screen.getByText('保存在这台手机上')).toBeOnTheScreen();
  expect(snippetRows()).toHaveLength(3);
  expect(screen.getByLabelText('编辑片段 docker ps')).toBeOnTheScreen();
  expect(screen.getByLabelText('编辑片段 tail -f api.log')).toBeOnTheScreen();
  expect(screen.getAllByText('直接执行')).toHaveLength(3);
});

it('validates an empty command and saves a new snippet on this phone', async () => {
  const rendered = await renderEditor();

  await press('添加片段');
  expect(screen.getByRole('header')).toHaveTextContent('添加片段');

  await press('保存');
  expect(screen.getByText('命令不能为空')).toBeOnTheScreen();
  expect(saved(rendered.kv)?.version).toBe(1);

  await fireEvent.changeText(screen.getByLabelText('命令'), 'htop');
  await press('保存');

  expect(saved(rendered.kv)).toEqual({
    version: 2,
    value: {
      v: 1,
      items: [
        ...initial,
        // T24: a new snippet only types its command until told otherwise.
        expect.objectContaining({ label: '', cmd: 'htop', run: false }),
      ],
    },
  });
  expect(snippetRows()).toHaveLength(4);
  expect(screen.getByLabelText('编辑片段 htop')).toBeOnTheScreen();
  // T23
  // The editor's modal hides its siblings from queries; the toast is one.
  expect(
    screen.getByTestId('toast', { includeHiddenElements: true }),
  ).toHaveTextContent('已保存');
});

it('takes a multi-line command', async () => {
  await renderEditor();
  await press('添加片段');
  // T11
  expect(screen.getByLabelText('命令')).toHaveProp('multiline', true);
});

it('turns off direct execution when editing a snippet', async () => {
  const rendered = await renderEditor();

  await press('编辑片段 tail -f api.log');
  expect(screen.getByRole('header')).toHaveTextContent('编辑片段');
  await press('点了直接执行');
  await press('保存');

  const value = saved(rendered.kv)?.value;
  expect(saved(rendered.kv)?.version).toBe(2);
  expect(
    value?.items.find(item => item.cmd === 'tail -f api.log'),
  ).toMatchObject({ run: false });
  expect(screen.getByText('只输入')).toBeOnTheScreen();
});

it('deletes a snippet after confirming the command in the dialog', async () => {
  await renderEditor();

  await press('编辑片段 docker ps');
  await press('删除片段 docker ps');
  expect(screen.getByText('删除这个片段？')).toBeOnTheScreen();
  expect(screen.getByText('docker ps')).toBeOnTheScreen();

  await press('删除');
  expect(snippetRows()).toHaveLength(2);
  expect(screen.queryByLabelText('编辑片段 docker ps')).toBeNull();
});

it('undoes a delete from the toast', async () => {
  const rendered = await renderEditor();
  await press('编辑片段 docker ps');
  await press('删除片段 docker ps');
  await press('删除');
  // T23: the delete says so and can be taken back.
  const hidden = { includeHiddenElements: true };
  expect(screen.getByTestId('toast', hidden)).toHaveTextContent(/已删除/);
  await fireEvent.press(screen.getByTestId('toast-action', hidden));
  await waitFor(() =>
    expect(screen.getByLabelText('编辑片段 docker ps')).toBeOnTheScreen(),
  );
  expect(snippetRows()).toHaveLength(3);
  expect(
    saved(rendered.kv)?.value.items.find(item => item.cmd === 'docker ps'),
  ).toMatchObject({ label: '', cmd: 'docker ps', run: true });
});

it('closes the editor from the sheet close button', async () => {
  const rendered = await renderEditor();
  const close = screen.getByTestId('snippets-editor-close');

  expect(close).toHaveProp('accessibilityLabel', common.a11y.close);
  await fireEvent.press(close);
  expect(rendered.onClose).toHaveBeenCalledTimes(1);
});

it('shows the empty state when no snippets are synced', async () => {
  await renderEditor([]);

  expect(screen.getByText('还没有命令片段')).toBeOnTheScreen();
  expect(
    screen.getByText('常用命令存成片段，在终端里点一下就能输入。'),
  ).toBeOnTheScreen();
});

it('disables adding once the snippet limit is reached', async () => {
  await renderEditor(
    Array.from({ length: SNIPPET_MAX }, (_, index) => ({
      ...initial[0],
      id: `s${index}`,
    })),
  );

  expect(screen.getByLabelText('添加片段')).toBeDisabled();
  expect(screen.getByText('最多 50 条')).toBeOnTheScreen();
});

it('reports a command over the character limit without writing', async () => {
  const rendered = await renderEditor();

  await press('添加片段');
  await fireEvent.changeText(
    screen.getByLabelText('命令'),
    'x'.repeat(SNIPPET_CMD_MAX + 1),
  );
  await press('保存');

  expect(
    screen.getByText(`命令太长了，最多 ${SNIPPET_CMD_MAX} 个字符`),
  ).toBeOnTheScreen();
  expect(saved(rendered.kv)?.version).toBe(1);
});

it('returns to the list without saving once the draft is discarded', async () => {
  const rendered = await renderEditor();

  await press('添加片段');
  await fireEvent.changeText(screen.getByLabelText('命令'), 'htop');
  await press('返回片段列表');
  // T10: a draft is not dropped silently.
  expect(
    screen.getByTestId('snippet-discard-dialog', HIDDEN),
  ).toBeOnTheScreen();
  await fireEvent.press(screen.getByTestId('snippet-discard-confirm', HIDDEN));

  expect(screen.getByRole('header')).toHaveTextContent('命令片段');
  expect(snippetRows()).toHaveLength(3);
  expect(saved(rendered.kv)?.version).toBe(1);
});

it('keeps the editor open and reports a failed write', async () => {
  await renderEditor(initial, kv =>
    failLocalSync(kv, 'snippets', new Error('offline')),
  );

  await press('添加片段');
  await fireEvent.changeText(screen.getByLabelText('命令'), 'htop');
  await press('保存');

  expect(screen.getByText('没保存上：offline')).toBeOnTheScreen();
  expect(screen.getByRole('header')).toHaveTextContent('添加片段');
});

it('keeps editing when the discard is declined, also from the close button', async () => {
  const rendered = await renderEditor();
  await press('添加片段');
  await fireEvent.changeText(screen.getByLabelText('命令'), 'htop');
  await fireEvent.press(screen.getByTestId('snippets-editor-close'));
  expect(
    screen.getByTestId('snippet-discard-dialog', HIDDEN),
  ).toBeOnTheScreen();
  expect(rendered.onClose).not.toHaveBeenCalled();
  await fireEvent.press(screen.getByText('继续编辑', HIDDEN));
  expect(screen.queryByTestId('snippet-discard-dialog', HIDDEN)).toBeNull();
  expect(screen.getByLabelText('命令')).toHaveProp('value', 'htop');

  await fireEvent.press(screen.getByTestId('snippets-editor-close'));
  await fireEvent.press(screen.getByTestId('snippet-discard-confirm', HIDDEN));
  expect(rendered.onClose).toHaveBeenCalledTimes(1);
});

it('leaves an untouched form without asking', async () => {
  await renderEditor();
  await press('编辑片段 docker ps');
  await press('返回片段列表');
  expect(screen.queryByTestId('snippet-discard-dialog', HIDDEN)).toBeNull();
  expect(screen.getByRole('header')).toHaveTextContent('命令片段');
});

it('shows a read failure with retry instead of an empty list', async () => {
  let failRead = true;
  await renderEditor(initial, kv => {
    const getJSON = kv.getJSON.bind(kv);
    jest.spyOn(kv, 'getJSON').mockImplementation(<T,>(key: string) => {
      if (failRead && key === LOCAL_SYNC_PREFIX + 'snippets') {
        throw new Error('storage locked');
      }
      return getJSON<T>(key);
    });
  });
  await waitFor(() =>
    expect(screen.getByText('片段没读出来')).toBeOnTheScreen(),
  );
  expect(screen.queryByText('还没有命令片段')).toBeNull();
  failRead = false;
  await fireEvent.press(screen.getByTestId('snippets-retry'));
  await waitFor(() => expect(snippetRows()).toHaveLength(3));
});
