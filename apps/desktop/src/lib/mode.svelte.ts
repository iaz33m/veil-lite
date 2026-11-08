// Light, dark or follow the system. Saved per machine; applied before first paint by public/mode.js.
export type Mode = "system" | "light" | "dark";

function read(): Mode {
  try {
    const m = localStorage.getItem("veil.mode");
    return m === "light" || m === "dark" ? m : "system";
  } catch {
    return "system";
  }
}

class ModeStore {
  mode = $state<Mode>(read());

  set(mode: Mode) {
    this.mode = mode;
    try {
      if (mode === "system") localStorage.removeItem("veil.mode");
      else localStorage.setItem("veil.mode", mode);
    } catch {
      // Storage can be unavailable; the choice still applies for this session.
    }
    if (mode === "system") delete document.documentElement.dataset.mode;
    else document.documentElement.dataset.mode = mode;
  }

  next() {
    this.set(this.mode === "system" ? "light" : this.mode === "light" ? "dark" : "system");
  }
}

export const colourMode = new ModeStore();
