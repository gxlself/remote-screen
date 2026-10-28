/** iPad 多窗口：每台电脑可以单独占一个系统窗口。 */
export const windows = {
  /** 切换浮层每台电脑一行右边的按钮。 */
  openNew: '在新窗口打开',
  /** 已经在别的窗口里显示的会话，列表里这样标。 */
  elsewhere: '在其他窗口',
  /** 附加窗口等主窗口准备好。 */
  loading: '正在准备',
  locked: {
    title: 'Remote Screen 已锁定',
    body: '在主窗口解锁后，这个窗口里的电脑才会显示。',
    action: '去主窗口',
  },
  a11y: {
    openNew: (name: string) => `在新窗口打开 ${name}`,
    elsewhere: '点按切到那个窗口',
  },
};
