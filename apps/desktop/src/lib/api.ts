// Typed wrappers over the Tauri commands in src-tauri/src/commands.rs.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { ActivityEntry, AppState, EngineEvent, ModelStatus, Permission, StatusInfo } from "./types";

/** Errors from commands arrive as plain strings. */
export function errorText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return "Something went wrong talking to Veil's engine.";
}

export const api = {
  status: () => invoke<StatusInfo>("status"),
  getPolicy: () => invoke<string>("get_policy"),
  setPolicy: (yaml: string) => invoke<void>("set_policy", { yaml }),
  pause: (minutes: number) => invoke<void>("pause", { minutes }),
  resume: () => invoke<void>("resume"),
  activity: (limit: number) => invoke<ActivityEntry[]>("activity", { limit }),
  clearActivity: () => invoke<void>("clear_activity"),
  modelStatus: () => invoke<ModelStatus>("model_status"),
  downloadModel: () => invoke<void>("download_model"),
  cancelDownload: () => invoke<void>("cancel_download"),
  removeModel: () => invoke<void>("remove_model"),
  permissions: () => invoke<Permission[]>("permissions"),
  requestPermission: (id: string) => invoke<boolean>("request_permission", { id }),
  engineRestart: () => invoke<void>("engine_restart"),
  appState: () => invoke<AppState>("app_state"),
  completeSetup: (autostart: boolean) => invoke<void>("complete_setup", { autostart }),
  setAutostart: (enabled: boolean) => invoke<void>("set_autostart", { enabled }),
  openLink: (which: "repository" | "privacy" | "security" | "license") => invoke<void>("open_link", { which }),
  checkForUpdate: () => invoke<string | null>("check_for_update"),
};

export function onEngineEvent(handler: (ev: EngineEvent) => void): Promise<UnlistenFn> {
  return listen<EngineEvent>("veil://event", (e) => handler(e.payload));
}

/** Fired by the Rust side when the engine connection drops or comes back. */
export function onEngineLink(handler: (up: boolean) => void): Promise<UnlistenFn> {
  return listen<boolean>("veil://engine-link", (e) => handler(e.payload));
}
