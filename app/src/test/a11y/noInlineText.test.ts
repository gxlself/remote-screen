import { readSource, sourceFiles, stripComments } from './helpers';

// Exceptions are exact source lines, never entire files. Each records why it
// cannot be moved by this task; new UI copy must still use zh/<page>.ts.
const ALLOWED_INLINE_TEXT: { file: string; text: string; reason: string }[] = [
  {
    file: 'features/directory/pathStore.ts',
    text: "throw new Error('useHostPath 必须在 EventsProvider 里使用');",
    reason:
      '内部 Provider / 设置错误信息；features 为本卡只读范围，待所属任务统一迁移。',
  },
  {
    file: 'features/events/EventsProvider.tsx',
    text: "throw new Error('useCoreEvent 必须在 EventsProvider 里使用');",
    reason:
      '内部 Provider / 设置错误信息；features 为本卡只读范围，待所属任务统一迁移。',
  },
  {
    file: 'features/files/fakeFiles.ts',
    text: "name: '桌面',",
    reason: 'U15 文件名和路径的假数据，模拟远端用户内容，不是界面固定文案。',
  },
  {
    file: 'features/files/fakeFiles.ts',
    text: "name: '文稿',",
    reason: 'U15 文件名和路径的假数据，模拟远端用户内容，不是界面固定文案。',
  },
  {
    file: 'features/files/fakeFiles.ts',
    text: "name: '下载',",
    reason: 'U15 文件名和路径的假数据，模拟远端用户内容，不是界面固定文案。',
  },
  {
    file: 'features/files/fakeFiles.ts',
    text: "path: '/Users/alice/桌面',",
    reason: 'U15 文件名和路径的假数据，模拟远端用户内容，不是界面固定文案。',
  },
  {
    file: 'features/files/fakeFiles.ts',
    text: "name: '设计稿',",
    reason: 'U15 文件名和路径的假数据，模拟远端用户内容，不是界面固定文案。',
  },
  {
    file: 'features/files/fakeFiles.ts',
    text: "name: '截屏 10.20.png',",
    reason: 'U15 文件名和路径的假数据，模拟远端用户内容，不是界面固定文案。',
  },
  {
    file: 'features/files/fakeFiles.ts',
    text: '[`${sourceKey(srcStudio)}|/Users/alice/桌面`]: desktopListing,',
    reason: 'U15 文件名和路径的假数据，模拟远端用户内容，不是界面固定文案。',
  },
  {
    file: 'features/files/fakeFiles.ts',
    text: "remotePath: '/Users/alice/桌面/build-0.3.1.dmg',",
    reason: 'U15 文件名和路径的假数据，模拟远端用户内容，不是界面固定文案。',
  },
  {
    file: 'features/files/fakeFiles.ts',
    text: "remotePath: '/Users/alice/桌面/截屏 10.20.png',",
    reason: 'U15 文件名和路径的假数据，模拟远端用户内容，不是界面固定文案。',
  },
  {
    file: 'features/files/fakeFiles.ts',
    text: "name: '季度报表-Q3.xlsx',",
    reason: 'U15 文件名和路径的假数据，模拟远端用户内容，不是界面固定文案。',
  },
  {
    file: 'features/files/filesApi.ts',
    text: "return new RsCoreError('unsupported', 'files: 待契约');",
    reason: 'U15 契约未接入的内部错误信息；features 为本卡只读范围。',
  },
  {
    file: 'features/network/store.ts',
    text: "throw new Error('useNetwork 必须在 EventsProvider 里使用');",
    reason:
      '内部 Provider / 设置错误信息；features 为本卡只读范围，待所属任务统一迁移。',
  },
  {
    file: 'features/session/sharedView.ts',
    text: "const marker = deviceName.lastIndexOf('的 ');",
    reason: '既有设备名称解析分隔符，属于逻辑而非界面文案；本卡禁止改逻辑。',
  },
  {
    file: 'features/session/store.ts',
    text: "throw new Error('useSessions 必须在 EventsProvider 里使用');",
    reason:
      '内部 Provider / 设置错误信息；features 为本卡只读范围，待所属任务统一迁移。',
  },
  {
    file: 'features/settings/store.ts',
    text: 'console.warn(`RsCore.settings.set 不支持 ${key}，设置只保存在本地`);',
    reason:
      '内部 Provider / 设置错误信息；features 为本卡只读范围，待所属任务统一迁移。',
  },
  {
    file: 'features/settings/store.ts',
    text: "throw new Error('useSettingsStore 必须在 SettingsProvider 里使用');",
    reason:
      '内部 Provider / 设置错误信息；features 为本卡只读范围，待所属任务统一迁移。',
  },
  {
    file: 'features/storage/StorageContext.tsx',
    text: "throw new Error('useKv 必须在 StorageProvider 里使用');",
    reason:
      '内部 Provider / 设置错误信息；features 为本卡只读范围，待所属任务统一迁移。',
  },
  {
    file: 'features/files/fakeFiles.ts',
    text: "localUri: 'file:///fake/截屏 10.20.png',",
    reason: 'U15 文件 URI 假数据，模拟远端用户内容，不是界面固定文案。',
  },
];

it('keeps Chinese UI copy in i18n, with file:line diagnostics', () => {
  const violations: string[] = [];
  const used = new Set<object>();
  const files = [
    ...sourceFiles('screens').filter(file => file.endsWith('.tsx')),
    ...sourceFiles('components').filter(file => file.endsWith('.tsx')),
    ...sourceFiles('features'),
  ];
  expect(files.length).toBeGreaterThan(0);
  for (const file of files) {
    stripComments(readSource(file))
      .split('\n')
      .forEach((line, index) => {
        if (!/[一-鿿]/.test(line)) {
          return;
        }
        const exception = ALLOWED_INLINE_TEXT.find(
          item => item.file === file && item.text === line.trim(),
        );
        if (exception !== undefined) {
          expect(exception.reason.trim()).not.toBe('');
          used.add(exception);
        } else {
          violations.push(`${file}:${index + 1}: ${line.trim()}`);
        }
      });
  }
  expect(violations).toEqual([]);
  expect(ALLOWED_INLINE_TEXT.filter(item => !used.has(item))).toEqual([]);
});

it('strips comments without losing lines, URLs, regexp or template text', () => {
  const source = [
    '// 注释',
    'const url = "https://hs.example.com/中文"; // 注释',
    '/* 块注释',
    '结束 */ const value = `中文 ${/* 注释 */ 1} // 中文`;',
    'const expression = /[一-鿿]/; /* 注释 */',
    'const element = <Text>中文 // 文字</Text>;',
  ].join('\n');
  const lines = stripComments(source).split('\n');
  expect(lines).toHaveLength(6);
  expect(lines[0].trim()).toBe('');
  expect(lines[1].trim()).toBe('const url = "https://hs.example.com/中文";');
  expect(lines[2].trim()).toBe('');
  expect(lines[3]).toContain('const value = `中文 ${');
  expect(lines[3]).toContain('1} // 中文`');
  expect(lines.join('\n')).not.toContain('注释');
  expect(lines[4].trim()).toBe('const expression = /[一-鿿]/;');
  expect(lines[5]).toContain('中文 // 文字');
});
