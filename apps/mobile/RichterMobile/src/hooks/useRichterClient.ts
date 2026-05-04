import { useCallback, useRef } from 'react';
import { useStore } from '../store/AppContext';

export function useRichterClient() {
  const { setConnection, updateNow, isConnected } = useStore();
  const wsRef = useRef<WebSocket | null>(null);

  const connect = useCallback(async (host: string, port: number) => {
    const baseUrl = `https://${host}:${port}/mobile/v1`;
    try {
      const resp = await fetch(`${baseUrl}/health`);
      const health = await resp.json();
      setConnection(true, health.daemon_id);

      const nowResp = await fetch(`${baseUrl}/now`);
      const now = await nowResp.json();
      updateNow(now);

      const ws = new WebSocket(`wss://${host}:${port}/mobile/v1/events/stream`);
      ws.onmessage = (event) => {
        const data = JSON.parse(event.data);
        if (data.importance >= 70) updateNow({ topEvent: data });
      };
      wsRef.current = ws;
    } catch {
      setConnection(false);
    }
  }, [setConnection, updateNow]);

  const disconnect = useCallback(() => {
    wsRef.current?.close();
    setConnection(false);
  }, [setConnection]);

  return { connect, disconnect, isConnected };
}
