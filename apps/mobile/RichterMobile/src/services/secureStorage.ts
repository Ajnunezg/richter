import * as SecureStore from 'expo-secure-store';

const KEYS = {
  DEVICE_ID: 'richter_device_id',
  DEVICE_KEY: 'richter_device_key',
  SERVER_FINGERPRINT: 'richter_server_fingerprint',
  SCOPES: 'richter_scopes',
  BASE_URL: 'richter_base_url',
} as const;

export const secureStorage = {
  async saveDeviceCredentials(id: string, key: string, fingerprint: string, scopes: string[]) {
    await SecureStore.setItemAsync(KEYS.DEVICE_ID, id);
    await SecureStore.setItemAsync(KEYS.DEVICE_KEY, key);
    await SecureStore.setItemAsync(KEYS.SERVER_FINGERPRINT, fingerprint);
    await SecureStore.setItemAsync(KEYS.SCOPES, JSON.stringify(scopes));
  },
  async getDeviceId() { return SecureStore.getItemAsync(KEYS.DEVICE_ID); },
  async getDeviceKey() { return SecureStore.getItemAsync(KEYS.DEVICE_KEY); },
  async getServerFingerprint() { return SecureStore.getItemAsync(KEYS.SERVER_FINGERPRINT); },
  async getScopes(): Promise<string[]> {
    const raw = await SecureStore.getItemAsync(KEYS.SCOPES);
    return raw ? JSON.parse(raw) : [];
  },
  async saveBaseUrl(url: string) { await SecureStore.setItemAsync(KEYS.BASE_URL, url); },
  async getBaseUrl() { return SecureStore.getItemAsync(KEYS.BASE_URL); },
  async clearAll() {
    for (const k of Object.values(KEYS)) await SecureStore.deleteItemAsync(k);
  },
};
