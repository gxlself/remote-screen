export const sessionViewport = {
  minimap: {
    label: (display: number | null, region: string) =>
      `画面位置：正在看${
        display === null ? '' : `显示器 ${display} 的`
      }${region}部分`,
  },
  followZoom: {
    tracking: '跟随输入位置',
  },
  regions: {
    topLeft: '左上',
    top: '上方',
    topRight: '右上',
    left: '左侧',
    middle: '中间',
    right: '右侧',
    bottomLeft: '左下',
    bottom: '下方',
    bottomRight: '右下',
  },
} as const;
