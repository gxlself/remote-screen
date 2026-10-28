export const coach = {
  trackpad: {
    title: '触控板模式：整块屏幕都是触控板',
    body: (mac: boolean) =>
      `手指不用对准，光标在哪就点哪。${
        mac ? '和 MacBook 触控板的手势一样。' : ''
      }之后在 更多 › 手势说明 里可以再看`,
    items: {
      move: { gesture: '单指滑动', effect: '移动光标，快划会加速' },
      tap: { gesture: '轻点', effect: '单击；轻点两下是双击' },
      secondary: { gesture: '双指轻点', effect: '右键菜单' },
      scroll: { gesture: '双指滑动', effect: '滚动页面，带惯性' },
      zoom: {
        gesture: '双指捏合',
        effect: '放大手机上的画面，不改电脑分辨率',
      },
      toolbar: { gesture: '三指轻点', effect: '显示或收起工具栏' },
      spaces: { gesture: '三指左右滑', effect: '切换同时连着的电脑' },
    },
    footer: [
      '长按半秒再拖动 = 按住鼠标拖拽、选中文字',
      '双指双击 = 切换触控板 / 触屏',
    ],
    switchLabel: '改用触屏',
  },
  touch: {
    title: '触屏模式：点哪里就点哪里',
    body: '手指点在哪，电脑上就点在哪。之后在 更多 › 手势说明 里可以再看',
    items: {
      drag: { gesture: '单指拖动', effect: '光标跟着手指走，按住拖动' },
      tap: { gesture: '单指轻点', effect: '在手指位置单击' },
      secondary: { gesture: '双指轻点', effect: '右键菜单' },
      scroll: { gesture: '双指拖动', effect: '滚动页面' },
      zoom: {
        gesture: '双指捏合',
        effect: '放大手机上的画面，不改电脑分辨率',
      },
      toolbar: { gesture: '三指轻点', effect: '显示或收起工具栏' },
      spaces: { gesture: '三指左右滑', effect: '切换同时连着的电脑' },
    },
    footer: [
      '长按再拖动 = 放大镜，松手处精确单击',
      '双指双击 = 切换触控板 / 触屏',
    ],
    switchLabel: '改用触控板',
  },
  start: '开始使用',
  screenReaderNote:
    '读屏开启时这些手势由读屏接管；可以用底部工具栏切换操作方式、打开键盘，在「更多」里还原缩放。',
  itemA11y: (gesture: string, effect: string) => `${gesture}：${effect}`,
} as const;
