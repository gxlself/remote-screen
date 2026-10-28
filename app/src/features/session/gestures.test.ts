import { coachContent } from './gestures';

const trackpad = [
  { gesture: '单指滑动', effect: '移动光标，快划会加速' },
  { gesture: '轻点', effect: '单击；轻点两下是双击' },
  { gesture: '双指轻点', effect: '右键菜单' },
  { gesture: '双指滑动', effect: '滚动页面，带惯性' },
  { gesture: '双指捏合', effect: '放大手机上的画面，不改电脑分辨率' },
  { gesture: '三指轻点', effect: '显示或收起工具栏' },
  { gesture: '三指左右滑', effect: '切换同时连着的电脑' },
];

const touch = [
  { gesture: '单指拖动', effect: '光标跟着手指走，按住拖动' },
  { gesture: '单指轻点', effect: '在手指位置单击' },
  { gesture: '双指轻点', effect: '右键菜单' },
  { gesture: '双指拖动', effect: '滚动页面' },
  { gesture: '双指捏合', effect: '放大手机上的画面，不改电脑分辨率' },
  { gesture: '三指轻点', effect: '显示或收起工具栏' },
  { gesture: '三指左右滑', effect: '切换同时连着的电脑' },
];

describe('coachContent', () => {
  it('matches the canvas copy for trackpad mode', () => {
    const content = coachContent('trackpad', 'macos');
    expect(content.title).toBe('触控板模式：整块屏幕都是触控板');
    expect(content.body).toBe(
      '手指不用对准，光标在哪就点哪。和 MacBook 触控板的手势一样。之后在 更多 › 手势说明 里可以再看',
    );
    expect(
      content.items.map(({ gesture, effect }) => ({ gesture, effect })),
    ).toEqual(trackpad);
    expect(content.footer).toEqual([
      '长按半秒再拖动 = 按住鼠标拖拽、选中文字',
      '双指双击 = 切换触控板 / 触屏',
    ]);
    expect(content.switchLabel).toBe('改用触屏');
    expect(content.switchTo).toBe('touch');
  });

  it('describes touch mode and switches back to trackpad', () => {
    const content = coachContent('touch', 'macos');
    expect(content.title).toBe('触屏模式：点哪里就点哪里');
    expect(content.body).toBe(
      '手指点在哪，电脑上就点在哪。之后在 更多 › 手势说明 里可以再看',
    );
    expect(
      content.items.map(({ gesture, effect }) => ({ gesture, effect })),
    ).toEqual(touch);
    expect(content.footer).toEqual([
      '长按再拖动 = 放大镜，松手处精确单击',
      '双指双击 = 切换触控板 / 触屏',
    ]);
    expect(content.switchLabel).toBe('改用触控板');
    expect(content.switchTo).toBe('trackpad');
  });

  it('omits the MacBook reference on Windows hosts', () => {
    const content = coachContent('trackpad', 'windows');
    expect(content.body).not.toContain('MacBook');
    expect(content.body).toBe(
      '手指不用对准，光标在哪就点哪。之后在 更多 › 手势说明 里可以再看',
    );
  });

  it('keeps the two-finger double tap hint in both modes', () => {
    for (const mode of ['trackpad', 'touch'] as const) {
      expect(coachContent(mode, 'macos').footer.join('\n')).toContain(
        '双指双击',
      );
    }
  });

  it('gives every item an icon', () => {
    for (const mode of ['trackpad', 'touch'] as const) {
      for (const item of coachContent(mode, 'macos').items) {
        expect(item.icon).not.toBe('');
      }
    }
  });
});
