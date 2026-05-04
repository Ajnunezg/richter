import { View, Text, TextInput, StyleSheet, Pressable } from 'react-native';
import { useState } from 'react';
import { colors } from '../../src/theme/colors';
import { GlassCard } from '../../src/components';

type PairState = 'scanning' | 'connecting' | 'paired' | 'error';

export default function PairingScreen() {
  const [state, setState] = useState<PairState>('scanning');
  const [host, setHost] = useState('');
  const [port, setPort] = useState('9777');
  const [secret, setSecret] = useState('');
  const [error, setError] = useState('');

  const connect = () => { setState('connecting'); setTimeout(() => setState('paired'), 1500); };

  return (
    <View style={styles.root}>
      {state === 'scanning' && (
        <GlassCard style={styles.card}>
          <Text style={styles.title}>Scan QR Code</Text>
          <View style={styles.cameraPlaceholder}>
            <Text style={styles.cameraText}>📷 Camera view here{'\n'}(expo-camera)</Text>
          </View>
          <Text style={styles.divider}>— or enter manually —</Text>
          <TextInput style={styles.input} placeholder="Host (e.g., 192.168.1.100)" placeholderTextColor={colors.textMuted} value={host} onChangeText={setHost} />
          <TextInput style={styles.input} placeholder="Port" placeholderTextColor={colors.textMuted} value={port} onChangeText={setPort} keyboardType="number-pad" />
          <TextInput style={styles.input} placeholder="Pairing secret" placeholderTextColor={colors.textMuted} value={secret} onChangeText={setSecret} secureTextEntry />
          <Pressable style={styles.btn} onPress={connect}><Text style={styles.btnText}>Connect</Text></Pressable>
        </GlassCard>
      )}
      {state === 'connecting' && (
        <GlassCard style={styles.card}><Text style={styles.title}>Connecting…</Text><Text style={styles.sub}>Establishing secure connection to your Mac</Text></GlassCard>
      )}
      {state === 'paired' && (
        <GlassCard style={styles.card}><Text style={styles.title}>✅ Paired!</Text><Text style={styles.sub}>Richter Mobile is now connected.{'\n'}You can close this screen.</Text></GlassCard>
      )}
      {state === 'error' && (
        <GlassCard style={styles.card}>
          <Text style={styles.title}>❌ Connection Failed</Text>
          <Text style={styles.sub}>{error || 'Could not connect. Check host and port.'}</Text>
          <Pressable style={[styles.btn, { backgroundColor: colors.danger }]} onPress={() => setState('scanning')}><Text style={styles.btnText}>Retry</Text></Pressable>
        </GlassCard>
      )}
    </View>
  );
}

const styles = StyleSheet.create({
  root: { flex: 1, backgroundColor: colors.background, justifyContent: 'center', padding: 20 },
  card: { padding: 24 },
  title: { color: colors.text, fontSize: 20, fontWeight: '700', textAlign: 'center', marginBottom: 12 },
  sub: { color: colors.textSecondary, fontSize: 14, textAlign: 'center' },
  cameraPlaceholder: { height: 200, backgroundColor: colors.surfaceElevated, borderRadius: 12, justifyContent: 'center', alignItems: 'center', marginVertical: 16 },
  cameraText: { color: colors.textMuted, textAlign: 'center', fontSize: 13 },
  divider: { color: colors.textMuted, textAlign: 'center', marginVertical: 16, fontSize: 13 },
  input: { backgroundColor: colors.surfaceElevated, color: colors.text, borderRadius: 8, padding: 12, fontSize: 14, marginBottom: 10, borderColor: colors.border, borderWidth: 1 },
  btn: { backgroundColor: colors.primary, borderRadius: 10, paddingVertical: 14, alignItems: 'center', marginTop: 8 },
  btnText: { color: '#fff', fontWeight: '600', fontSize: 15 },
});
