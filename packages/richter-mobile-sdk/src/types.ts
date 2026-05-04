export interface RichterMobileConfig {
  baseUrl: string;
  serverFingerprint?: string;
  deviceId?: string;
  deviceKey?: string;
  scopes?: string[];
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

export interface MobileStatusResponse {
  mobile_gateway: boolean;
  lan_gateway: boolean;
  paired_devices: number;
  active_pairing_sessions: number;
}

export interface RunSummary {
  id: string; command: string; repo: string; status: string;
  exit_code?: number; fingerprint: string; is_cached: boolean; subscribers: number;
}

export interface AgentInfo {
  id: string; name: string; agent_type: string; cwd: string;
  repo_id: string; active_command?: string;
}

export interface ApprovalRequest {
  approval_id: string; risk_level: string; command: string; repo: string;
  requesting_agent: string; reason: string; expires_at: string; consequences: string;
}

export interface PairingSession {
  pairing_id: string; pairing_secret: string; server_pubkey_sha256: string;
  daemon_id: string; host: string; port: number; expires_in_seconds: number;
}
