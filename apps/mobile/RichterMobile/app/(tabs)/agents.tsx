import { FlatList, StyleSheet } from 'react-native';
import { AgentCard } from '../../src/components/AgentCard';
import { colors } from '../../src/theme/colors';
import type { AgentInfo } from '../../src/types';

const MOCK: AgentInfo[] = [
  { id: '1', name: 'Claude Code', agent_type: 'claude', cwd: '/Projects/imaginethat-cli', repo_id: 'r1', active_command: 'cargo test', claimed_paths: [] },
  { id: '2', name: 'Codex CLI', agent_type: 'codex', cwd: '/Projects/imaginethat-cli', repo_id: 'r1', claimed_paths: [] },
  { id: '3', name: 'Droid', agent_type: 'droid', cwd: '/Projects/richter', repo_id: 'r2', active_command: 'cargo build', claimed_paths: [] },
];

export default function AgentsScreen() {
  return (
    <FlatList data={MOCK} keyExtractor={(a) => a.id} style={styles.root} renderItem={({ item }) => <AgentCard agent={item} />} />
  );
}

const styles = StyleSheet.create({ root: { flex: 1, backgroundColor: colors.background, padding: 12 } });
