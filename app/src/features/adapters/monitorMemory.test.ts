import { MemoryKv } from '../storage/kv';
import { rememberedMonitors, rememberMonitors } from './monitorMemory';

const office = {
  protocol: 'rdp',
  hostId: 'w3k8',
  addr: '100.64.0.12',
} as const;

it('remembers the count per computer and defaults to one', () => {
  const kv = new MemoryKv();
  expect(rememberedMonitors(kv, office)).toBe(1);
  rememberMonitors(kv, office, 2);
  expect(rememberedMonitors(kv, office)).toBe(2);
  // Same computer at another address: still the same entry.
  expect(rememberedMonitors(kv, { ...office, addr: '198.51.100.20' })).toBe(2);
  expect(rememberedMonitors(kv, { ...office, hostId: 'other' })).toBe(1);
});

it('keys typed addresses by address and port', () => {
  const kv = new MemoryKv();
  const typed = { protocol: 'rdp', addr: '192.0.2.5' } as const;
  rememberMonitors(kv, typed, 3);
  expect(rememberedMonitors(kv, { ...typed, port: 3389 })).toBe(3);
  expect(rememberedMonitors(kv, { ...typed, port: 3390 })).toBe(1);
});

it('ignores bad values and VNC', () => {
  const kv = new MemoryKv();
  kv.setJSON('adapters.monitors.rdp:w3k8', 9);
  expect(rememberedMonitors(kv, office)).toBe(1);
  rememberMonitors(kv, { ...office, protocol: 'vnc' }, 2);
  expect(rememberedMonitors(kv, { ...office, protocol: 'vnc' })).toBe(1);
});
