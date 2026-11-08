import type { ActivityEntry, Channel, ModelStatus, Outcome } from "./types";

export const CHANNEL_LABEL: Record<Channel, string> = {
  message: "Message",
  paste: "Paste",
  drop: "Drag and drop",
  upload: "File upload",
  typing: "Typing",
};

export const OUTCOME_LABEL: Record<Outcome, string> = {
  allowed: "Checked, nothing found",
  sent_anyway: "You sent it anyway",
  redacted: "Redacted, then sent",
  cancelled: "You didn’t send it",
  blocked: "Blocked by policy",
  drop_blocked: "Drop stopped",
  unchecked: "Couldn’t be checked",
};

/** Outcomes where Veil held something back. */
export const HELD: ReadonlySet<Outcome> = new Set(["cancelled", "blocked", "drop_blocked", "redacted"]);

const DETECTION_LABEL: Record<string, string> = {
  keyword: "Keyword",
  email_address: "Email address",
  phone_number: "Phone number",
  uk_phone_number: "Phone number",
  uk_mobile_number: "Phone number",
  credit_card: "Card number",
  iban: "IBAN",
  iban_code: "IBAN",
  us_ssn: "US SSN",
  uk_nino: "UK NI number",
  uk_nhs: "NHS number",
  ip_address: "IP address",
  private_key: "Private key",
  jwt_token: "Access token",
  generic_secret: "Secret",
  es_dni: "Spanish DNI",
  es_nie: "Spanish NIE",
  de_tax_id: "German tax ID",
  fr_nir: "French NIR",
  it_fiscal_code: "Italian fiscal code",
  nl_bsn: "Dutch BSN",
  eu_vat: "EU VAT number",
  in_aadhaar: "Aadhaar",
  in_pan: "Indian PAN",
  pk_cnic: "Pakistani CNIC",
  au_tfn: "Australian TFN",
  sg_nric: "Singapore NRIC",
  "ai:secrets": "Secrets (AI)",
  "ai:personal_info": "Personal info (AI)",
  "ai:confidential": "Confidential (AI)",
  "ai:source_code": "Source code (AI)",
};

export function detectionLabel(type: string): string {
  const known = DETECTION_LABEL[type];
  if (known) return known;
  if (type.endsWith("_api_key") || type.endsWith("_token") || type.endsWith("_access_key")) return "API key or token";
  const words = type.replace(/^ai:/, "").replace(/_/g, " ");
  return words.charAt(0).toUpperCase() + words.slice(1);
}

export function appLabel(e: Pick<ActivityEntry, "app_name" | "site">): string {
  return e.site ? `${e.app_name}, ${e.site}` : e.app_name;
}

const timeFmt = new Intl.DateTimeFormat(undefined, { hour: "2-digit", minute: "2-digit" });
const dayFmt = new Intl.DateTimeFormat(undefined, { day: "numeric", month: "short" });

export function clock(ms: number): string {
  return timeFmt.format(new Date(ms));
}

export function when(ms: number, now = Date.now()): string {
  const d = new Date(ms);
  const today = new Date(now);
  const sameDay = d.toDateString() === today.toDateString();
  return sameDay ? clock(ms) : `${dayFmt.format(d)} ${clock(ms)}`;
}

export function isToday(ms: number, now = Date.now()): boolean {
  return new Date(ms).toDateString() === new Date(now).toDateString();
}

/** `14:05` for minutes and seconds left. */
export function countdown(secs: number): string {
  const s = Math.max(0, Math.round(secs));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const r = s % 60;
  const mm = String(m).padStart(h > 0 ? 2 : 1, "0");
  const ss = String(r).padStart(2, "0");
  return h > 0 ? `${h}:${mm}:${ss}` : `${mm}:${ss}`;
}

export function modelSummary(m: ModelStatus): string {
  switch (m.state) {
    case "missing":
      return "Not downloaded. Keyword and pattern checks are on.";
    case "downloading":
      return `Downloading, ${m.percent}%`;
    case "ready":
      return "Ready. Loads when needed.";
    case "loaded":
      return "Running on this computer.";
    case "failed":
      return `Not available: ${m.reason}`;
    case "disabled":
      return "Not included in this build.";
  }
}

export function platformLabel(p: string): string {
  switch (p) {
    case "macos":
      return "macOS";
    case "windows":
      return "Windows";
    case "linux-x11":
      return "Linux (X11)";
    case "linux-wayland":
      return "Linux (Wayland)";
    default:
      return p;
  }
}
