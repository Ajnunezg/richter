import { ScrollView, Text, StyleSheet } from 'react-native';
import { useStore } from '../../src/store/AppContext';
import { GlassCard } from '../../src/components/GlassCard';
import { colors } from '../../src/theme/colors';

export default function SettingsScreen() {
  const { isConnected, daemonId, deviceName, scopes, isPaired, disconnect } = useStore();

  return (
    <ScrollView style={styles.root}>
      <GlassCard style={styles.section}>
        <Text style={styles.sectionTitle}>Connection</Text>
        <Text style={styles.label}>Status: {isConnected ? 'Connected' : 'Disconnected'}</Text>
        <Text style={styles.label}>Daemon: {daemonId ?? 'N/A'}</Text>
      </GlassCard>
      <GlassCard style={styles.section}>
        <Text style={styles.sectionTitle}>Device</Text>
        <Text style={styles.label}>Name: {deviceName ?? 'Not paired'}</Text>
        <Text style={styles.label}>Scopes: {scopes.length > 0 ? scopes.join(', ') : 'None'}</Text>
      </GlassCard>
      <GlassCard style={styles.section}>
        <Text style={styles.sectionTitle}>Notifications</Text>
        <Text style={styles.label}>Push: Not configured</Text>
      </GlassCard>
      {isPaired && (
        <GlassCard style={styles.section}>
          <Text style={styles.sectionTitle}>Security</Text>
          <Text style={styles.dangerText} onPress={disconnect}>Revoke This Device</Text>
        </GlassCard>
      )}
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  root: { flex: 1, backgroundColor: colors.background, padding: 12 },
  section: { marginBottom: 12 },
  sectionTitle: { color: colors.textSecondary, fontSize: 11, fontWeight: '600', textTransform: 'uppercase', marginBottom: 8, letterSpacing: 1 },
  label: { color: colors.text, fontSize: 14, marginBottom: 4 },
  dangerText: { color: colors.danger, fontSize: 14, fontWeight: '600', marginTop: 4 },
});
