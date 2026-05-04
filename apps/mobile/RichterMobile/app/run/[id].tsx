import { View, Text, ScrollView, StyleSheet, Pressable } from 'react-native';
import { useLocalSearchParams } from 'expo-router';
import { GlassCard } from '../../src/components';
import { colors } from '../../src/theme/colors';

export default function RunDetailScreen() {
  const { id } = useLocalSearchParams<{ id: string }>();
  return (
    <ScrollView style={styles.root}>
      <GlassCard style={styles.card}><Text style={styles.label}>Run ID</Text><Text style={styles.value}>{id}</Text></GlassCard>
      <GlassCard style={styles.card}><Text style={styles.label}>Command</Text><Text style={styles.mono}>cargo test --workspace</Text></GlassCard>
      <GlassCard style={styles.card}><Text style={styles.label}>Status</Text><View style={styles.badge}><Text style={styles.badgeText}>running</Text></View></GlassCard>
      <GlassCard style={styles.card}>
        <Text style={styles.label}>Output Preview (redacted)</Text>
        <Text style={styles.log}>Running tests...{'\n'}PASS test_auth{'\n'}FAIL test_refresh: [REDACTED]</Text>
        <Pressable style={styles.fetchBtn}><Text style={styles.fetchBtnText}>Fetch Full Log</Text></Pressable>
      </GlassCard>
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  root: { flex: 1, backgroundColor: colors.background, padding: 12 },
  card: { marginBottom: 12 },
  label: { color: colors.textSecondary, fontSize: 11, fontWeight: '600', textTransform: 'uppercase', marginBottom: 4 },
  value: { color: colors.text, fontSize: 15 },
  mono: { color: colors.text, fontSize: 13, fontFamily: 'monospace' },
  badge: { backgroundColor: colors.warning, borderRadius: 4, paddingHorizontal: 8, paddingVertical: 3, alignSelf: 'flex-start' },
  badgeText: { color: '#000', fontSize: 11, fontWeight: '700', textTransform: 'uppercase' },
  log: { color: colors.textSecondary, fontSize: 12, fontFamily: 'monospace', backgroundColor: colors.surfaceElevated, padding: 10, borderRadius: 8, marginTop: 4 },
  fetchBtn: { marginTop: 10, backgroundColor: colors.primary, borderRadius: 8, paddingVertical: 10, alignItems: 'center' },
  fetchBtnText: { color: '#fff', fontWeight: '600', fontSize: 13 },
});
