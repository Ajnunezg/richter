import { useEffect, useRef } from 'react';
import { useStore } from '../store/AppContext';

export function useEventStream() {
  const updateNow = useStore((s) => s.updateNow);
  const wsRef = useRef<WebSocket | null>(null);

  const start = (baseUrl: string) => {
    const ws = new WebSocket(`${baseUrl}/events/stream`);
    ws.onmessage = (event) => {
      const data = JSON.parse(event.data);
      if (data.type === 'important_event' && data.importance >= 70) {
        updateNow({ topEvent: data });
      }
    };
    wsRef.current = ws;
  };

  const stop = () => wsRef.current?.close();

  useEffect(() => () => stop(), []);

  return { start, stop };
}
