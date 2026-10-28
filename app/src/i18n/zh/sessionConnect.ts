import { common } from './common';

export const sessionConnect = {
  connectingTitle: (name: string) => `正在连接 ${name}`,
  stepList: '连接步骤',
  steps: {
    tunnel: { name: '接通电脑', secondary: '网络' },
    auth: { name: '确认身份', secondary: '电脑上的 Host' },
    firstFrame: { name: '等待画面', secondary: '视频' },
  },
  stepSeconds: (seconds: number) => `${Math.floor(seconds)} 秒`,
  stepA11y: (
    name: string,
    state: 'done' | 'active' | 'pending',
    seconds: number | null,
  ) =>
    seconds === null
      ? `${name}，${common.step[state]}`
      : `${name}，${common.step[state]}，${Math.floor(seconds)} 秒`,
  stuckA11y: (detail: string, hint: string) => `${detail}。${hint}`,
  stuck: {
    firstFrame: {
      detail: '还在等画面：Mac 可能刚从睡眠里醒来',
      windowsDetail: '还在等画面：电脑可能刚从睡眠里醒来',
      hint: '不用操作，画面一到就自动进入',
    },
    tunnel: {
      detail: '还在接通：正在换一条路试',
      windowsDetail: '还在接通：正在换一条路试',
      hint: '不用操作，连上就自动进入',
    },
    auth: {
      detail: '还在确认身份：电脑上的 Remote Host 响应慢',
      windowsDetail: '还在确认身份：电脑上的 Remote Host 响应慢',
      hint: '不用操作，确认后就自动进入',
    },
    elapsed: (seconds: number, detail: string) =>
      `已等 ${Math.floor(seconds)} 秒，${detail}`,
  },
  slow: {
    title: '画面一直没出来',
    body: '可以重新连一次，或者先降低画质试试。',
    retry: '重新连接',
    lowerQuality: '降低画质',
  },
  collapse: '收起',
  collapseA11y: '收起，在后台继续连接',
  approval: {
    macTitle: '等待 Mac 上允许',
    windowsTitle: '等待电脑上允许',
    body: (name: string) =>
      `${name} 屏幕上弹出了确认窗口，需要电脑前的人点「允许」`,
  },
} as const;
