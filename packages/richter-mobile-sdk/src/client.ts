import type { RichterMobileConfig, MobileNowResponse, MobileStatusResponse, MobileEvent, RunSummary, AgentInfo, ApprovalRequest } from './types';

export class RichterMobileClient {
  private baseUrl: string;
  private deviceId?: string;
  private deviceKey?: string;
  private eventStream: EventSource | null = null;

  constructor(config: RichterMobileConfig) {
    this.baseUrl = config.baseUrl.replace(/\/$/, '');
    this.deviceId = config.deviceId;
    this.deviceKey = config.deviceKey;
  }

  private async request<T>(path: string, options?: RequestInit): Promise<T> {
    const headers: Record<string, string> = { 'Content-Type': 'application/json' };
    if (this.deviceId) headers['X-Device-Id'] = this.deviceId;
    if (this.deviceKey) headers['Authorization'] = `Bearer ${this.deviceKey}`;
    const resp = await fetch(`${this.baseUrl}${path}`, { ...options, headers });
    if (!resp.ok) throw new Error(`Richter API error: ${resp.status}`);
    return resp.json();
  }

  async health(): Promise<{ status: string; daemon_id: string }> { return this.request('/mobile/v1/health'); }
  async getNow(): Promise<MobileNowResponse> { return this.request('/mobile/v1/now'); }
  async getStatus(): Promise<MobileStatusResponse> { return this.request('/mobile/v1/status'); }
  async getRuns(): Promise<RunSummary[]> { return this.request('/mobile/v1/runs'); }
  async getAgents(): Promise<AgentInfo[]> { return this.request('/mobile/v1/agents'); }
  async getImportantEvents(): Promise<MobileEvent[]> { return this.request('/mobile/v1/events/important'); }
  async getApprovals(): Promise<ApprovalRequest[]> { return this.request('/mobile/v1/approvals'); }
  async approveRequest(approvalId: string): Promise<{ status: string }> { return this.request(`/mobile/v1/approvals/${approvalId}/approve`, { method: 'POST' }); }
  async denyRequest(approvalId: string): Promise<{ status: string }> { return this.request(`/mobile/v1/approvals/${approvalId}/deny`, { method: 'POST' }); }

  streamImportantEvents(onEvent: (event: MobileEvent) => void, minImportance = 70): () => void {
    const url = `${this.baseUrl}/mobile/v1/events/stream?min_importance=${minImportance}`;
    const es = new EventSource(url);
    es.onmessage = (msg) => { try { const e: MobileEvent = JSON.parse(msg.data); if (e.importance >= minImportance) onEvent(e); } catch { /* skip */ } };
    es.onerror = () => es.close();
    this.eventStream = es;
    return () => es.close();
  }

  disconnect() { this.eventStream?.close(); this.eventStream = null; }
}
