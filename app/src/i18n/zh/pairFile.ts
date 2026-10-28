export const pairFile = {
  title: '导入配对文件',
  intro:
    '在「文件」App 或 AirDrop 里点电脑导出的 .rspair 文件，或者把文件内容粘贴到下面。',
  introAndroid:
    '点电脑导出的 rsp:// 配对链接，或者把 .rspair 文件的内容粘贴到下面。',
  input: {
    label: '配对文件内容',
    placeholder: 'rsp://pairfile?...',
  },
  buttons: {
    paste: '从剪贴板粘贴',
    read: '读取',
    pair: '配对',
  },
  clipboardEmpty: '剪贴板是空的',
  clipboardError: '读不了剪贴板，请手动粘贴',
  summary: {
    computer: '电脑',
    fingerprint: '指纹',
    address: '地址',
    validUntil: (dateTime: string) => `${dateTime} 前有效`,
    expired: '已过期',
    expiredHint: '在电脑上重新导出一份',
  },
  warning: '只导入你自己的电脑导出的文件。配对后这台手机就能连那台电脑。',
  errors: {
    notPairFile: '这不是 Remote Screen 的配对文件',
    unsupported: '没法直接读取这个文件，请在文件里长按 › 拷贝内容后粘贴到这里',
    generic: '读不了这个文件，请检查内容后再试',
    retry: '重试',
  },
  unknownHost: '未知电脑',
  a11y: {
    close: '关闭',
    summary: '配对文件摘要',
    summaryText: ({
      hostName,
      expired,
      fp,
      addr,
      validUntil,
    }: {
      hostName: string;
      expired: boolean;
      fp: string;
      addr?: string;
      validUntil: string;
    }): string =>
      `${pairFile.a11y.summary}，电脑 ${hostName}，指纹 ${fp
        .split(' ')
        .join('，')}${
        addr === undefined ? '' : `，地址 ${addr}`
      }，${validUntil}${expired ? `，${pairFile.summary.expired}` : ''}`,
  },
} as const;
