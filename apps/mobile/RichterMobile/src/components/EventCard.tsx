import { View, Text, StyleSheet } from 'react-native';
import { colors } from '../theme/colors';
import type { MobileEvent } from '../types';

function importanceColor(v: number) {
  if (v >= 90) return colors.importance.critical;
  if (v >= 70) return colors.importance.high;
  return colors.importance.normal;
}

export function EventCard({ event }: { event: MobileEvent }) {
  const minsAgo = Math.round((Date.now() - new Date(event.occurred_at).getTime()) / 60000);
  return (
    <View style={styles.card}>
      <View style={styles.header}>
        <View style={[styles.badge, { backgroundColor: importanceColor(event.importance) }]}>
          <Text style={styles.badgeText}>{event.importance}</Text>
        </View>
        <Text style={styles.title} numberOfLines={1}>{event.title}</Text>
        {event.requires_action && <Text style={styles.action}>⚡</Text>}
      </View>
      <Text style={styles.summary} numberOfLines={2}>{event.summary}</Text>
      <Text style={styles.meta}>{minsAgo}m ago{event.repo_id ? ` · ${event.repo_id}` : ''}</Text>
    </View>
  );
}

const styles = StyleSheet.create({
  card: { backgroundColor: colors.surface, borderColor: colors.border, borderWidth: 1, borderRadius: 10, padding: 12, marginBottom: 8 },
  header: { flexDirection: 'row', alignItems: 'center', marginBottom: 6 },
  badge: { borderRadius: 6, paddingHorizontal: 6, paddingVertical: 2, marginRight: 8 },
  badgeText: { color: '#fff', fontSize: 11, fontWeight: '700' },
  title: { color: colors.text, fontSize: 14, fontWeight: '600', flex: 1 },
  action: { fontSize: 14, marginLeft: 4 },
  summary: { color: colors.textSecondary, fontSize: 12, marginBottom: 4 },
  meta: { color: colors.textMuted, fontSize: 11 },
});
