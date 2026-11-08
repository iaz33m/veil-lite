// Read and write policy.yaml as a typed object. The engine validates on save and is the source of truth.
import { parse, stringify } from "yaml";
import type { CheckKey, Policy, Region } from "./types";

export const CHECKS: { key: CheckKey; title: string; detail: string }[] = [
  { key: "secrets", title: "Passwords, keys and secrets", detail: "API keys, tokens, passwords and private keys." },
  { key: "personal_info", title: "Personal information", detail: "Names with addresses, IDs, health or money details about a person." },
  { key: "confidential", title: "Confidential business information", detail: "Plans, figures, contracts and anything marked internal." },
  { key: "source_code", title: "Proprietary source code", detail: "Code that looks like it belongs to your company." },
];

export const REGIONS: { key: Region; title: string; detail: string }[] = [
  { key: "universal", title: "Everywhere", detail: "Email addresses, phone numbers, card numbers, IBANs, API keys" },
  { key: "us", title: "United States", detail: "Social Security numbers and other US identifiers" },
  { key: "uk", title: "United Kingdom", detail: "National Insurance and NHS numbers, sort codes" },
  { key: "eu", title: "European Union", detail: "Spanish DNI/NIE, German tax ID, French NIR, Italian fiscal code, Dutch BSN, VAT numbers" },
  { key: "apac", title: "Asia-Pacific", detail: "Aadhaar, PAN, CNIC, Australian TFN, Singapore NRIC" },
];

/** Defaults matching `Policy::default()`, used to fill gaps in a partial file. */
export function defaultPolicy(): Policy {
  return {
    version: 1,
    scope: { mode: "all_apps", exclude_apps: [], include_apps: [], include_sites: [] },
    keywords: ["confidential"],
    pii: { enabled: true, regions: ["universal", "us", "uk", "eu", "apac"], min_score: 0.5 },
    checks: {
      secrets: { threshold: 0.6, action: "ask", enabled: true },
      personal_info: { threshold: 0.7, action: "ask", enabled: true },
      confidential: { threshold: 0.7, action: "ask", enabled: true },
      source_code: { threshold: 0.75, action: "ask", enabled: true },
    },
    actions: { default: "ask", allow_redact: true, fail_mode: { text: "open", files: "closed" } },
    pause: { allowed: true },
    activity_log: { enabled: true },
    model: { enabled: true, max_chunks: 32, idle_unload_secs: 300, prescore_delay_ms: 400 },
    send_labels: [],
    pick_labels: [],
  };
}

function isObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

function merge<T>(base: T, over: unknown): T {
  if (!isObject(base) || !isObject(over)) return (over === undefined ? base : (over as T));
  const out: Record<string, unknown> = { ...(base as Record<string, unknown>) };
  for (const [k, v] of Object.entries(over)) {
    out[k] = k in out ? merge(out[k], v) : v;
  }
  return out as T;
}

export function parsePolicy(yaml: string): Policy {
  const raw: unknown = parse(yaml);
  return merge(defaultPolicy(), raw ?? {});
}

export function stringifyPolicy(policy: Policy): string {
  return stringify(policy, { lineWidth: 0 });
}

/** One entry per line, trimmed, blanks and duplicates dropped. */
export function linesToList(text: string): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const line of text.split("\n")) {
    const v = line.trim();
    if (v && !seen.has(v.toLowerCase())) {
      seen.add(v.toLowerCase());
      out.push(v);
    }
  }
  return out;
}
