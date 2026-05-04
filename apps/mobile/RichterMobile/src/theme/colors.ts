export const colors = {
  background: '#0a0a0f',
  surface: '#0f0f1a',
  surfaceElevated: '#16162a',
  border: '#1e293b',
  primary: '#6366f1',
  primaryLight: '#818cf8',
  success: '#22c55e',
  warning: '#f59e0b',
  danger: '#ef4444',
  text: '#e2e8f0',
  textSecondary: '#64748b',
  textMuted: '#475569',
  importance: { low: '#64748b', normal: '#3b82f6', high: '#f59e0b', critical: '#ef4444' },
  agentColors: {
    claude: '#d97706', codex: '#6366f1', droid: '#22c55e',
    forge: '#ec4899', kimi: '#06b6d4', minimax: '#8b5cf6', other: '#64748b',
  },
} as const;
