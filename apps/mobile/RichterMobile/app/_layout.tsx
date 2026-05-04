import { Stack } from 'expo-router';

export default function RootLayout() {
  return (
    <Stack
      screenOptions={{
        headerStyle: { backgroundColor: '#0a0a0f' },
        headerTintColor: '#e2e8f0',
        contentStyle: { backgroundColor: '#0a0a0f' },
        animation: 'slide_from_right',
      }}
    >
      <Stack.Screen name="(tabs)" options={{ headerShown: false }} />
      <Stack.Screen name="pairing" options={{ title: 'Pair with Mac', presentation: 'modal' }} />
      <Stack.Screen name="run/[id]" options={{ title: 'Run Detail' }} />
    </Stack>
  );
}
