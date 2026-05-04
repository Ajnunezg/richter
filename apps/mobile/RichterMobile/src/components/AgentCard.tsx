import { View, Text, StyleSheet } from 'react-native';
import { colors } from '../theme/colors';
import type { AgentInfo } from '../types';

function agentAccent(t: string) {
  return (colors.agentColors as Record<string, string>)[t] ?? colors.agentColors.other;
}

export function AgentCard({ agent }: { agent: AgentInfo }) {
  const accent = agentAccent(agent.agent_type);
  return (
    <View style={[styles.card, { borderLeftColor: accent, borderLeftWidth: 3 }]}>
      <Text style={styles.name}>{agent.name}</Text>
      <Text style={styles.type}>{agent.agent_type}</Text>
      <Text style={styles.cwd} numberOfLines={1}>{agent.cwd}</Text>
      {agent.active_command && (
        <View style={styles.cmdPill}><Text style={styles.cmdText} numberOfLines={1}>{agent.active_command}</Text></View>
      )}
    </View>
  );
}

const styles = StyleSheet.create({
  card: { backgroundColor: colors.surface, borderColor: colors.border, borderWidth: 1, borderRadius: 10, padding: 12, marginBottom: 8 },
  name: { color: colors.text, fontSize: 14, fontWeight: '600' },
  type: { color: colors.textMuted, fontSize: 11, marginBottom: 4 },
  cwd: { color: colors.textSecondary, fontSize: 11, fontFamily: 'monospace' },
  cmdPill: { backgroundColor: '#1e3a5f', borderRadius: 4, paddingHorizontal: 8, paddingVertical: 2, marginTop: 6, alignSelf: 'flex-start' },
  cmdText: { color: '#3b82f6', fontSize: 10, fontFamily: 'monospace' },
});
