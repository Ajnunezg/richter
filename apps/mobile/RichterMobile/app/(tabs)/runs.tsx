import { FlatList, StyleSheet } from 'react-native';
import { RunCard } from '../../src/components/RunCard';
import { colors } from '../../src/theme/colors';
import type { RunSummary } from '../../src/types';

const MOCK: RunSummary[] = [
  { id: '1', command: 'cargo test', repo: 'imaginethat-cli', status: 'running', fingerprint: 'sha256:abc', is_cached: false, subscribers: 2 },
  { id: '2', command: 'pnpm lint', repo: 'imaginethat-cli', status: 'cached', exit_code: 0, fingerprint: 'sha256:def', is_cached: true, subscribers: 1 },
  { id: '3', command: 'cargo build', repo: 'richter', status: 'running', fingerprint: 'sha256:ghi', is_cached: false, subscribers: 1 },
];

export default function RunsScreen() {
  return (
    <FlatList data={MOCK} keyExtractor={(r) => r.id} style={styles.root} renderItem={({ item }) => <RunCard run={item} onPress={() => {}} />} />
  );
}

const styles = StyleSheet.create({ root: { flex: 1, backgroundColor: colors.background, padding: 12 } });
