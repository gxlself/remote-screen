import type { ComponentType } from 'react';

import AboutSection from './about';
import DevicesSection from './devices';
import GeneralSection from './general';
import HistorySection from './history';
import NetworkSection from './network';
import PairingSection from './pairing';
import PermissionsSection from './permissions';

export type SectionId =
  'general' | 'network' | 'devices' | 'pairing' | 'permissions' | 'history' | 'about';

export const SETTINGS_SECTIONS: ReadonlyArray<{
  id: SectionId;
  component: ComponentType;
}> = [
  { id: 'general', component: GeneralSection },
  { id: 'network', component: NetworkSection },
  { id: 'devices', component: DevicesSection },
  { id: 'pairing', component: PairingSection },
  { id: 'permissions', component: PermissionsSection },
  { id: 'history', component: HistorySection },
  { id: 'about', component: AboutSection },
];
