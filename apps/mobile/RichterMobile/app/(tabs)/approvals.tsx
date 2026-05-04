import { FlatList, Text, StyleSheet } from 'react-native';
import { ApprovalCard } from '../../src/components/ApprovalCard';
import { colors } from '../../src/theme/colors';
import type { ApprovalRequest } from '../../src/types';

const MOCK: ApprovalRequest[] = [
  {
    approval_id: '1', risk_level: 'high', command: 'rm -rf node_modules', repo: 'imaginethat-cli',
    requesting_agent: 'Claude Code', reason: 'Clean install requested',
    expires_at: new Date(Date.now() + 45000).toISOString(),
    consequences: 'Will delete all node_modules. Re-install takes ~3 minutes.',
  },
];

export default function ApprovalsScreen() {
  return (
    <FlatList
      data={MOCK} keyExtractor={(a) => a.approval_id} style={styles.root}
      ListEmptyComponent={<Text style={styles.empty}>No pending approvals</Text>}
      renderItem={({ item }) => <ApprovalCard approval={item} onApprove={() => {}} onDeny={() => {}} />}
    />
  );
}

const styles = StyleSheet.create({
  root: { flex: 1, backgroundColor: colors.background, padding: 12 },
  empty: { color: colors.textSecondary, textAlign: 'center', marginTop: 40 },
});
