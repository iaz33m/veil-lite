// Live engine state for every screen: status, pause countdown, model progress and recent activity.
import { api, errorText, onEngineEvent, onEngineLink } from "./api";
import type { ActivityEntry, EngineEvent, ModelStatus, StatusInfo } from "./types";

const ACTIVITY_LIMIT = 300;
const POLL_MS = 5000;

class EngineStore {
  status = $state<StatusInfo | null>(null);
  /** Set while the engine cannot be reached. */
  error = $state<string | null>(null);
  loading = $state(true);
  pausedLeft = $state<number | null>(null);
  model = $state<ModelStatus | null>(null);
  /** Last download percentage seen, kept after a pause so the UI can say where it stopped. */
  downloadedPercent = $state<number | null>(null);
  warning = $state<string | null>(null);

  activity = $state<ActivityEntry[]>([]);
  activityState = $state<"idle" | "loading" | "ready" | "error">("idle");
  activityError = $state<string | null>(null);
  policyVersion = $state(0);

  private started = false;

  start() {
    if (this.started) return;
    this.started = true;
    void this.refresh();
    void onEngineEvent((ev) => this.onEvent(ev));
    void onEngineLink((up) => {
      if (up) void this.refresh();
      else this.error = "Veil’s engine stopped responding.";
    });
    setInterval(() => void this.refresh(), POLL_MS);
    setInterval(() => {
      if (this.pausedLeft !== null) {
        this.pausedLeft = this.pausedLeft > 1 ? this.pausedLeft - 1 : null;
      }
    }, 1000);
  }

  async refresh() {
    try {
      const s = await api.status();
      this.status = s;
      this.setModel(s.model);
      this.pausedLeft = s.paused_remaining_secs;
      this.error = null;
    } catch (e) {
      this.error = errorText(e);
    } finally {
      this.loading = false;
    }
  }

  private setModel(m: ModelStatus) {
    this.model = m;
    if (m.state === "downloading") this.downloadedPercent = m.percent;
    if (m.state === "ready" || m.state === "loaded") this.downloadedPercent = null;
  }

  private onEvent(ev: EngineEvent) {
    switch (ev.event) {
      case "activity":
        this.activity = [ev.entry, ...this.activity].slice(0, ACTIVITY_LIMIT);
        break;
      case "paused":
        this.pausedLeft = ev.remaining_secs;
        break;
      case "resumed":
        this.pausedLeft = null;
        break;
      case "model":
        this.setModel(ev.status);
        break;
      case "policy_changed":
        this.policyVersion += 1;
        break;
      case "warning":
        this.warning = ev.message;
        break;
    }
  }

  async loadActivity(force = false) {
    if (this.activityState === "loading" || (this.activityState === "ready" && !force)) return;
    this.activityState = "loading";
    try {
      this.activity = await api.activity(ACTIVITY_LIMIT);
      this.activityState = "ready";
      this.activityError = null;
    } catch (e) {
      this.activityError = errorText(e);
      this.activityState = "error";
    }
  }

  async pause(minutes: number) {
    await api.pause(minutes);
    this.pausedLeft = minutes * 60;
  }

  async resume() {
    await api.resume();
    this.pausedLeft = null;
  }
}

export const engine = new EngineStore();
