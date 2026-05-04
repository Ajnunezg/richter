import { View, Text, FlatList, StyleSheet } from 'react-native';
import { GlassCard } from '../../src/components/GlassCard';
import { colors } from '../../src/theme/colors';

const MOCK_REPOS = [
  { id: '1', name: 'imaginethat-cli', branch: 'main', dirty: true, agents: 3, active: 2, queued: 1 },
  { id: '2', name: 'richter', branch: 'main', dirty: false, agents: 2, active: 1, queued: 0 },
];

export default function ReposScreen() {
  return (
    <FlatList
      data={MOCK_REPOS}
      keyExtractor={(r) => r.id}
      style={styles.root}
      renderItem={({ item }) => (
        <GlassCard style={styles.card}>
          <View style={styles.row}>
            <View style={[styles.dot, { backgroundColor: item.dirty ? colors.warning : colors.success }]} />
            <Text style={styles.name}>{item.name}</Text>
            <Text style={styles.branch}>{item.branch}</Text>
          </View>
          <Text style={styles.meta}>{item.agents} agents · {item.active} active · {item.queued} queued</Text>
        </GlassCard>
      )}
    />
  );
}

const styles = StyleSheet.create({
  root: { flex: 1, backgroundColor: colors.background, padding: 12 },
  card: { marginBottom: 8 },
  row: { flexDirection: 'row', alignItems: 'center', marginBottom: 4 },
  dot: { width: 8, height: 8, borderRadius: 4, marginRight: 8 },
  name: { color: colors.text, fontSize: 14, fontWeight: '600', flex: 1 },
  branch: { color: colors.textMuted, fontSize: 11 },
  meta: { color: colors.textSecondary, fontSize: 11, marginLeft: 16 },
});
