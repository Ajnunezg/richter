import { create } from 'zustand';
import type { MobileEvent } from '../types';

interface RichterState {
  isConnected: boolean;
  daemonId: string | null;
  deviceId: string | null;
  deviceName: string | null;
  scopes: string[];
  activeRuns: number;
  queuedRuns: number;
  cpuPercent: number;
  memoryPercent: number;
  topEvent: MobileEvent | null;
  approvalsPending: number;
  isPaired: boolean;
  setConnection: (connected: boolean, daemonId?: string) => void;
  setPairing: (deviceId: string, name: string, scopes: string[]) => void;
  updateNow: (data: Partial<RichterState>) => void;
  disconnect: () => void;
}

export const useStore = create<RichterState>((set) => ({
  isConnected: false,
  daemonId: null,
  deviceId: null,
  deviceName: null,
  scopes: [],
  activeRuns: 0,
  queuedRuns: 0,
  cpuPercent: 0,
  memoryPercent: 0,
  topEvent: null,
  approvalsPending: 0,
  isPaired: false,
  setConnection: (connected, daemonId) => set({ isConnected: connected, ...(daemonId ? { daemonId } : {}) }),
  setPairing: (deviceId, name, scopes) => set({ isPaired: true, deviceId, deviceName: name, scopes }),
  updateNow: (data) => set(data),
  disconnect: () => set({ isConnected: false, isPaired: false }),
}));

export function AppProvider({ children }: { children: React.ReactNode }) {
  return <>{children}</>;
}
