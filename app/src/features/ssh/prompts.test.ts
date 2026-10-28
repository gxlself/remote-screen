import {
  authUrlHost,
  checkSteps,
  formatFp,
  spokenFp,
  sshServerName,
} from './prompts';
import { sshServers } from '../../native/rscore/fixtures';

it('groups a SHA256 fingerprint in four-character chunks', () => {
  expect(formatFp('SHA256:nT4kP0aZWq8c')).toEqual([
    'SHA256:',
    'nT4k',
    'P0aZ',
    'Wq8c',
  ]);
});

it('groups an unprefixed fingerprint without adding a prefix', () => {
  expect(formatFp('abcdEFgh')).toEqual(['abcd', 'EFgh']);
});

it('joins the spoken fingerprint groups with Chinese commas', () => {
  expect(spokenFp('SHA256:nT4kP0aZWq8c')).toBe('SHA256:，nT4k，P0aZ，Wq8c');
});

it('extracts the host from a valid URL', () => {
  expect(authUrlHost('https://hs.example.com/a/xyz')).toBe('hs.example.com');
});

it('returns the original string when the URL cannot be parsed', () => {
  expect(authUrlHost('not a url')).toBe('not a url');
});

it('uses a done / active / pending sequence while waiting', () => {
  expect(checkSteps('wait', 'hs.example.com').map(step => step.state)).toEqual([
    'done',
    'active',
    'pending',
  ]);
  expect(checkSteps('ask', 'hs.example.com').map(step => step.state)).toEqual([
    'active',
    'pending',
    'pending',
  ]);
});

it('labels the browser step and the wait suffix', () => {
  expect(checkSteps('ask', 'hs.example.com')[1].title).toBe(
    '在浏览器登录 hs.example.com',
  );
  expect(checkSteps('wait', 'hs.example.com')[1].title).toBe(
    '在浏览器登录 hs.example.com · 等你登录',
  );
});

it('resolves a server display name and falls back to its id', () => {
  expect(sshServerName(sshServers, 'gpu-train-02')).toBe('gpu-train-02');
  expect(sshServerName(sshServers, 'missing-server')).toBe('missing-server');
  expect(sshServerName(undefined, 'missing-server')).toBe('missing-server');
});
