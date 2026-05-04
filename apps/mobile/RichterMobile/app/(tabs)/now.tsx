import { ScrollView, RefreshControl, Text, StyleSheet } from 'react-native';
import { useState } from 'react';
import { useStore } from '../../src/store/AppContext';
import { ConnectionBadge } from '../../src/components/ConnectionBadge';
import { PressureGauge } from '../../src/components/PressureGauge';
import { GlassCard } from '../../src/components/GlassCard';
import { EventCard } from '../../src/components/EventCard';
import { colors } from '../../src/theme/colors';

export default function NowScreen() {
  const { isConnected, daemonId, cpuPercent, memoryPercent, activeRuns, queuedRuns, topEvent, approvalsPending } = useStore();
  const [refreshing, setRefreshing] = useState(false);

  const onRefresh = async () => {
    setRefreshing(true);
    await new Promise((r) => setTimeout(r, 800));
    setRefreshing(false);
  };

  return (
    <ScrollView
      style={styles.root}
      refreshControl={<RefreshControl refreshing={refreshing} onRefresh={onRefresh} tintColor={colors.primary} />}
    >
      <ConnectionBadge isConnected={isConnected} daemonName={daemonId} />

      <GlassCard style={styles.section}>
        <Text style={styles.sectionTitle}>System Pressure</Text>
        <PressureGauge label="CPU" percent={cpuPercent} />
        <PressureGauge label="Memory" percent={memoryPercent} />
      </GlassCard>

      <GlassCard style={styles.section}>
        <Text style={styles.sectionTitle}>Activity</Text>
        <Text style={styles.stat}>Active Runs: {activeRuns}</Text>
        <Text style={styles.stat}>Queued: {queuedRuns}</Text>
        <Text style={styles.stat}>Pending Approvals: {approvalsPending}</Text>
      </GlassCard>

      {topEvent && (
        <GlassCard style={styles.section}>
          <Text style={styles.sectionTitle}>Top Event</Text>
          <EventCard event={topEvent} />
        </GlassCard>
      )}
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  root: { flex: 1, backgroundColor: colors.background, padding: 12 },
  section: { marginBottom: 12 },
  sectionTitle: {
    color: colors.textSecondary, fontSize: 11, fontWeight: '600',
    textTransform: 'uppercase', marginBottom: 8, letterSpacing: 1,
  },
  stat: { color: colors.text, fontSize: 14, marginBottom: 4 },
});
