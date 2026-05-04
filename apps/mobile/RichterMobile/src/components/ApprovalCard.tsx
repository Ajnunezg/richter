import { View, Text, StyleSheet, Pressable } from 'react-native';
import { colors } from '../theme/colors';
import type { ApprovalRequest } from '../types';

function riskColor(r: string) {
  switch (r) { case 'critical': return colors.danger; case 'high': return '#f59e0b'; case 'medium': return '#3b82f6'; default: return colors.success; }
}

export function ApprovalCard({ approval, onApprove, onDeny }: { approval: ApprovalRequest; onApprove: () => void; onDeny: () => void }) {
  return (
    <View style={styles.card}>
      <View style={styles.header}>
        <View style={[styles.risk, { backgroundColor: riskColor(approval.risk_level) }]}>
          <Text style={styles.riskText}>{approval.risk_level}</Text>
        </View>
        <Text style={styles.cmd} numberOfLines={1}>{approval.command}</Text>
      </View>
      <Text style={styles.meta}>{approval.repo} · {approval.requesting_agent}</Text>
      <Text style={styles.reason}>{approval.reason}</Text>
      <Text style={styles.expiry}>Expires in {Math.max(0, Math.round((new Date(approval.expires_at).getTime() - Date.now()) / 1000))}s</Text>
      <View style={styles.actions}>
        <Pressable onPress={onDeny} style={[styles.btn, { backgroundColor: colors.danger }]}>
          <Text style={styles.btnText}>Deny</Text>
        </Pressable>
        <Pressable onPress={onApprove} style={[styles.btn, { backgroundColor: colors.success }]}>
          <Text style={styles.btnText}>Approve</Text>
        </Pressable>
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  card: { backgroundColor: colors.surfaceElevated, borderColor: colors.border, borderWidth: 1, borderRadius: 10, padding: 14, marginBottom: 10 },
  header: { flexDirection: 'row', alignItems: 'center', marginBottom: 6 },
  risk: { borderRadius: 4, paddingHorizontal: 8, paddingVertical: 2, marginRight: 8 },
  riskText: { color: '#fff', fontSize: 10, fontWeight: '700', textTransform: 'uppercase' },
  cmd: { color: colors.text, fontSize: 13, fontFamily: 'monospace', flex: 1 },
  meta: { color: colors.textMuted, fontSize: 11, marginBottom: 4 },
  reason: { color: colors.textSecondary, fontSize: 12, marginBottom: 6 },
  expiry: { color: colors.warning, fontSize: 11, marginBottom: 10 },
  actions: { flexDirection: 'row', gap: 10 },
  btn: { flex: 1, borderRadius: 8, paddingVertical: 10, alignItems: 'center' },
  btnText: { color: '#fff', fontWeight: '600', fontSize: 14 },
});
