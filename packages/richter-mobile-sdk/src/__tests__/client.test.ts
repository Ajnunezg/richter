import { RichterMobileClient } from '../client';

describe('RichterMobileClient', () => {
  it('constructs with base URL', () => { const c = new RichterMobileClient({ baseUrl: 'https://192.168.1.100:9777' }); expect(c).toBeDefined(); });
  it('strips trailing slash', () => { const c = new RichterMobileClient({ baseUrl: 'https://192.168.1.100:9777/' }); expect(c).toBeDefined(); });
  it('accepts device credentials', () => { const c = new RichterMobileClient({ baseUrl: 'https://192.168.1.100:9777', deviceId: 'mob_001', deviceKey: 'dk_test' }); expect(c).toBeDefined(); });
  it('disconnect is safe when no stream', () => { const c = new RichterMobileClient({ baseUrl: 'https://192.168.1.100:9777' }); expect(() => c.disconnect()).not.toThrow(); });
});
