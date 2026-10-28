import {
  derpName,
  embeddedAddr,
  embeddedRunning,
  firstIpv4,
  peerForAddr,
  peerPath,
} from './peers';
import {
  routeEmbeddedNeedsLogin,
  routeEmbeddedRunning,
  tailnetRunning,
} from '../../native/rscore/fixtures';

describe('tailnet peer helpers', () => {
  it('selects the embedded IPv4 only while running', () => {
    expect(embeddedAddr(routeEmbeddedRunning)).toBe('100.64.0.21');
    expect(embeddedRunning(routeEmbeddedRunning)).toBe(true);
    expect(embeddedAddr(routeEmbeddedNeedsLogin)).toBeNull();
    expect(
      embeddedAddr({ ...routeEmbeddedRunning, active: 'system' }),
    ).toBeNull();
    expect(firstIpv4(['fd7a:115c:a1e0::15', '100.64.0.21'])).toBe(
      '100.64.0.21',
    );
    expect(embeddedAddr(null)).toBeNull();
    expect(embeddedRunning(undefined)).toBe(false);
    expect(firstIpv4(['100.64.0.256', 'fd7a:115c:a1e0::15'])).toBeNull();
  });

  it('classifies peer paths and unknown peers', () => {
    expect(peerPath(peerForAddr(tailnetRunning, '100.64.0.12'))).toBe('relay');
    expect(peerPath(peerForAddr(tailnetRunning, '100.64.0.7'))).toBe('direct');
    expect(peerForAddr(tailnetRunning, '100.64.0.99')).toBeNull();
    expect(peerPath(null)).toBe('unknown');
    const offline = {
      ...tailnetRunning.peers[0],
      online: false,
    };
    expect(peerPath(offline)).toBe('offline');
    expect(
      peerPath({ ...offline, online: true, direct: false, relay: '' }),
    ).toBe('unknown');
    expect(peerForAddr(null, '100.64.0.12')).toBeNull();
  });

  it('uses the local DERP region name and preserves unknown codes', () => {
    expect(derpName(tailnetRunning, 'sha')).toBe('上海');
    expect(derpName(tailnetRunning, 'fra')).toBe('fra');
    expect(derpName(null, '')).toBe('');
  });
});
