import { View, Text, StyleSheet, Pressable } from 'react-native';
import { colors } from '../theme/colors';
import type { RunSummary } from '../types';

function statusColor(s: string) {
  switch (s) { case 'running': return colors.warning; case 'cached': return '#3b82f6'; case 'failed': return colors.danger; default: return colors.textSecondary; }
}

export function RunCard({ run, onPress }: { run: RunSummary; onPress?: () => void }) {
  return (
    <Pressable onPress={onPress} style={styles.card}>
      <View style={styles.row}>
        <View style={[styles.dot, { backgroundColor: statusColor(run.status) }]} />
        <Text style={styles.command} numberOfLines={1}>{run.command}</Text>
        {run.is_cached && <View style={styles.cachedPill}><Text style={styles.cachedText}>cached</Text></View>}
      </View>
      <Text style={styles.meta}>{run.repo} · {run.status} · {run.subscribers} subs</Text>
    </Pressable>
  );
}

const styles = StyleSheet.create({
  card: { backgroundColor: colors.surface, borderColor: colors.border, borderWidth: 1, borderRadius: 10, padding: 12, marginBottom: 8 },
  row: { flexDirection: 'row', alignItems: 'center', marginBottom: 4 },
  dot: { width: 8, height: 8, borderRadius: 4, marginRight: 8 },
  command: { color: colors.text, fontSize: 13, fontFamily: 'monospace', flex: 1 },
  cachedPill: { backgroundColor: '#1e3a5f', borderRadius: 4, paddingHorizontal: 6, paddingVertical: 1 },
  cachedText: { color: '#3b82f6', fontSize: 10, fontWeight: '600' },
  meta: { color: colors.textMuted, fontSize: 11, marginLeft: 16 },
});
