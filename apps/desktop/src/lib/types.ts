// TypeScript mirror of the engine protocol (crates/veil-ipc/src/protocol.rs and veil-core types).
// Field names follow the Rust serde output exactly.

export type ModelStatus =
  | { state: "missing" }
  | { state: "downloading"; percent: number }
  | { state: "ready" }
  | { state: "loaded" }
  | { state: "failed"; reason: string }
  | { state: "disabled" };

export type Channel = "message" | "paste" | "drop" | "upload" | "typing";

export type Outcome =
  | "allowed"
  | "sent_anyway"
  | "redacted"
  | "cancelled"
  | "blocked"
  | "drop_blocked"
  | "unchecked";

export interface ActivityEntry {
  ts_ms: number;
  app_id: string;
  app_name: string;
  site: string | null;
  channel: Channel;
  detections: string[];
  outcome: Outcome;
  files: number;
}

export interface Permission {
  id: string;
  title: string;
  granted: boolean;
  help: string;
}

export interface StatusInfo {
  engine_version: string;
  platform: string;
  active: boolean;
  paused_remaining_secs: number | null;
  model: ModelStatus;
  permissions_ok: boolean;
  drops_blockable: boolean;
  ocr_available: boolean;
}

export type EngineEvent =
  | { event: "activity"; entry: ActivityEntry }
  | { event: "paused"; remaining_secs: number }
  | { event: "resumed" }
  | { event: "policy_changed" }
  | { event: "model"; status: ModelStatus }
  | { event: "warning"; message: string };

/** Desktop-app state kept by the Rust side (not the engine). */
export interface AppState {
  setup_complete: boolean;
  autostart: boolean;
  version: string;
}

export type Action = "allow" | "ask" | "block";
export type FailMode = "open" | "closed";
export type ScopeMode = "all_apps" | "listed_apps";
export type CheckKey = "secrets" | "personal_info" | "confidential" | "source_code";
export type Region = "universal" | "us" | "uk" | "eu" | "apac";

export interface CheckRule {
  threshold: number;
  action: Action;
  enabled: boolean;
}

/** policy.yaml (crates/veil-core/src/policy.rs). */
export interface Policy {
  version: number;
  scope: { mode: ScopeMode; exclude_apps: string[]; include_apps: string[]; include_sites: string[] };
  keywords: string[];
  pii: { enabled: boolean; regions: Region[]; min_score: number };
  checks: Record<CheckKey, CheckRule>;
  actions: { default: Action; allow_redact: boolean; fail_mode: { text: FailMode; files: FailMode } };
  pause: { allowed: boolean };
  activity_log: { enabled: boolean };
  model: { enabled: boolean; max_chunks: number; idle_unload_secs: number; prescore_delay_ms: number };
  send_labels: string[];
  pick_labels: string[];
}
