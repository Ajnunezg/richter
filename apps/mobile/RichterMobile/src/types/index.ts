export interface MobileEvent {
  event_id: string;
  type: string;
  importance: number;
  repo_id?: string;
  run_id?: string;
  title: string;
  summary: string;
  occurred_at: string;
  requires_action: boolean;
}

export interface MobileNowResponse {
  daemon_ok: boolean;
  active_runs: number;
  queued_runs: number;
  cpu_percent: number;
  memory_percent: number;
  top_event: MobileEvent | null;
  duplicate_work_saved: number;
  agent_conflicts: number;
  approvals_pending: number;
}

export interface RunSummary {
  id: string;
  command: string;
  repo: string;
  status: 'running' | 'queued' | 'cached' | 'completed' | 'failed';
  exit_code?: number;
  fingerprint: string;
  is_cached: boolean;
  subscribers: number;
  start_time?: string;
  duration_ms?: number;
}

export interface AgentInfo {
  id: string;
  name: string;
  agent_type: string;
  cwd: string;
  repo_id: string;
  active_command?: string;
  claimed_paths: string[];
}

export interface ApprovalRequest {
  approval_id: string;
  risk_level: 'low' | 'medium' | 'high' | 'critical';
  command: string;
  repo: string;
  requesting_agent: string;
  reason: string;
  expires_at: string;
  consequences: string;
}
