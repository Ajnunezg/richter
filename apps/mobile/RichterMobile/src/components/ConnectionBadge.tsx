import { View, Text, StyleSheet, Pressable } from 'react-native';
import { colors } from '../theme/colors';

interface Props {
  isConnected: boolean;
  daemonName?: string | null;
  onPress?: () => void;
}

export function ConnectionBadge({ isConnected, daemonName, onPress }: Props) {
  return (
    <Pressable onPress={onPress} style={styles.container}>
      <View style={[styles.dot, { backgroundColor: isConnected ? colors.success : colors.danger }]} />
      <Text style={styles.text}>
        {isConnected ? `Connected to ${daemonName ?? 'Mac'}` : 'Disconnected'}
      </Text>
    </Pressable>
  );
}

const styles = StyleSheet.create({
  container: { flexDirection: 'row', alignItems: 'center', padding: 8 },
  dot: { width: 10, height: 10, borderRadius: 5, marginRight: 8 },
  text: { color: colors.textSecondary, fontSize: 13 },
});
