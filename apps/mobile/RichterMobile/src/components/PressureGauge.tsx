import { View, Text, StyleSheet } from 'react-native';
import { colors } from '../theme/colors';

interface Props {
  label: string;
  percent: number;
}

function gaugeColor(p: number) {
  if (p > 85) return colors.danger;
  if (p > 60) return colors.warning;
  return colors.success;
}

export function PressureGauge({ label, percent }: Props) {
  return (
    <View style={styles.container}>
      <Text style={styles.label}>{label}</Text>
      <View style={styles.bar}>
        <View style={[styles.fill, { width: `${Math.min(100, percent)}%`, backgroundColor: gaugeColor(percent) }]} />
      </View>
      <Text style={styles.value}>{percent.toFixed(0)}%</Text>
    </View>
  );
}

const styles = StyleSheet.create({
  container: { marginBottom: 12 },
  label: { color: colors.textSecondary, fontSize: 12, marginBottom: 4 },
  bar: { height: 6, backgroundColor: colors.border, borderRadius: 3, overflow: 'hidden' },
  fill: { height: 6, borderRadius: 3 },
  value: { color: colors.text, fontSize: 14, fontWeight: '600', marginTop: 2 },
});
