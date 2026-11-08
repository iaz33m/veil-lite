// Hash router. The route and its filters live in the URL so a reload keeps the place.
export type Page = "setup" | "status" | "activity" | "settings" | "about";

const PAGES: readonly Page[] = ["setup", "status", "activity", "settings", "about"];

function parse(hash: string): { page: Page; sub: string; query: Record<string, string> } {
  const raw = hash.replace(/^#\/?/, "");
  const [path = "", qs = ""] = raw.split("?");
  const [first = "", sub = ""] = path.split("/");
  const page = (PAGES as readonly string[]).includes(first) ? (first as Page) : "status";
  const query: Record<string, string> = {};
  new URLSearchParams(qs).forEach((v, k) => (query[k] = v));
  return { page, sub, query };
}

function build(page: Page, sub = "", query: Record<string, string> = {}): string {
  const qs = new URLSearchParams(Object.entries(query).filter(([, v]) => v !== "")).toString();
  return `#/${page}${sub ? `/${sub}` : ""}${qs ? `?${qs}` : ""}`;
}

class Router {
  page = $state<Page>("status");
  sub = $state("");
  query = $state<Record<string, string>>({});

  constructor() {
    this.read();
    window.addEventListener("hashchange", () => this.read());
  }

  private read() {
    const r = parse(window.location.hash);
    this.page = r.page;
    this.sub = r.sub;
    this.query = r.query;
  }

  go(page: Page, sub = "", query: Record<string, string> = {}) {
    window.location.hash = build(page, sub, query);
  }

  /** Change the query (filters) without adding a history entry. */
  replaceQuery(query: Record<string, string>) {
    history.replaceState(null, "", build(this.page, this.sub, query));
    this.query = Object.fromEntries(Object.entries(query).filter(([, v]) => v !== ""));
  }

  href(page: Page, sub = ""): string {
    return build(page, sub);
  }
}

export const router = new Router();
