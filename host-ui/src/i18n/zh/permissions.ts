const permissions = {
  title: '权限',
  description: 'Remote Host 需要这两项系统权限才能工作。权限归属「Remote Host」，只要授权一次。',
  items: {
    screen_recording: {
      name: '屏幕录制',
      description: '读取屏幕画面，发给你的手机',
    },
    accessibility: {
      name: '辅助功能',
      description: '把手机上的点按和键盘输入转给这台 Mac',
    },
  },
  granted: '已授权',
  notGranted: '未授权',
  openSystemSettings: '打开系统设置',
  allGranted: '都已授权，可以被远程控制了',
  needsRestart: '授权后 macOS 可能会要求重新打开 Remote Host',
  rerunSetup: '重新运行首次设置…',
} as const;

export default permissions;
