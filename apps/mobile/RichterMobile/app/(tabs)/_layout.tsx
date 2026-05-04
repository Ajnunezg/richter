import { Tabs } from 'expo-router';
import { Ionicons } from '@expo/vector-icons';

export default function TabLayout() {
  return (
    <Tabs
      screenOptions={{
        tabBarActiveTintColor: '#6366f1',
        tabBarInactiveTintColor: '#64748b',
        tabBarStyle: { backgroundColor: '#0f0f1a', borderTopColor: '#1e293b', borderTopWidth: 1 },
        headerStyle: { backgroundColor: '#0a0a0f' },
        headerTintColor: '#e2e8f0',
      }}
    >
      <Tabs.Screen name="now" options={{ title: 'Now', tabBarIcon: ({ color, size }) => <Ionicons name="pulse" size={size} color={color} /> }} />
      <Tabs.Screen name="repos" options={{ title: 'Repos', tabBarIcon: ({ color, size }) => <Ionicons name="git-branch" size={size} color={color} /> }} />
      <Tabs.Screen name="runs" options={{ title: 'Runs', tabBarIcon: ({ color, size }) => <Ionicons name="play-circle" size={size} color={color} /> }} />
      <Tabs.Screen name="agents" options={{ title: 'Agents', tabBarIcon: ({ color, size }) => <Ionicons name="people" size={size} color={color} /> }} />
      <Tabs.Screen name="approvals" options={{ title: 'Approvals', tabBarIcon: ({ color, size }) => <Ionicons name="shield-checkmark" size={size} color={color} /> }} />
      <Tabs.Screen name="settings" options={{ title: 'Settings', tabBarIcon: ({ color, size }) => <Ionicons name="settings-outline" size={size} color={color} /> }} />
    </Tabs>
  );
}
