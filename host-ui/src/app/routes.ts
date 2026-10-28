import type { ComponentType } from 'react';

import BgPage from '../pages/bg';
import BorderPage from '../pages/border';
import CapsulePage from '../pages/capsule';
import ConfirmPage from '../pages/confirm';
import DevPage from '../pages/dev';
import MenuPage from '../pages/menu';
import PairPage from '../pages/pair';
import SettingsPage from '../pages/settings';
import SetupPage from '../pages/setup';
import UpdatePage from '../pages/update';

import type { PageId, PageProps } from './router';

export const ROUTES: Record<PageId, { component: ComponentType<PageProps>; title: string }> = {
  bg: { component: BgPage, title: '后台' },
  menu: { component: MenuPage, title: '菜单' },
  capsule: { component: CapsulePage, title: '正在被控制' },
  border: { component: BorderPage, title: '边框' },
  confirm: { component: ConfirmPage, title: '连接确认' },
  pair: { component: PairPage, title: '配对新设备' },
  settings: { component: SettingsPage, title: '设置' },
  setup: { component: SetupPage, title: '首次设置' },
  update: { component: UpdatePage, title: '软件更新' },
  dev: { component: DevPage, title: '开发索引' },
};
