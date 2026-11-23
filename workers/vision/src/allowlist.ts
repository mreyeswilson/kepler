/**
 * URL allowlist. Default: only the local machine (localhost, 127.0.0.1, [::1], *.localhost) over
 * http(s). Extra hosts must be configured explicitly (KEPLER_VISION_ALLOW_HOSTS, comma separated;
 * exact names or `*.suffix`). The Rust manager applies the same rules before sending a request;
 * the worker applies them again, to the page URL and to every sub-request and redirect.
 */

export type UrlCheck = { ok: true; url: URL } | { ok: false; reason: string };

export function parseAllowedHosts(raw: string | undefined): string[] {
  if (raw === undefined) return [];
  return raw
    .split(",")
    .map((h) => h.trim().toLowerCase())
    .filter((h) => h.length > 0 && h.length <= 253 && /^[a-z0-9.*_-]+$/.test(h))
    .slice(0, 32);
}

function isLocalHost(host: string): boolean {
  return host === "localhost" || host === "127.0.0.1" || host === "[::1]" || host.endsWith(".localhost");
}

function hostAllowed(host: string, extra: readonly string[]): boolean {
  if (isLocalHost(host)) return true;
  return extra.some((e) => (e.startsWith("*.") ? host.endsWith(e.slice(1)) : host === e));
}

export function checkUrl(raw: string, extra: readonly string[] = []): UrlCheck {
  let url: URL;
  try {
    url = new URL(raw);
  } catch {
    return { ok: false, reason: "not a valid URL" };
  }
  if (url.protocol !== "http:" && url.protocol !== "https:") return { ok: false, reason: `scheme ${url.protocol} is not allowed` };
  if (url.username !== "" || url.password !== "") return { ok: false, reason: "credentials in the URL are not allowed" };
  if (!hostAllowed(url.hostname.toLowerCase(), extra)) return { ok: false, reason: "host is not on the allowlist" };
  return { ok: true, url };
}

/** Sub-resources: the same allowlist, plus inline `data:` and `about:blank`. Everything else is aborted. */
export function subRequestAllowed(raw: string, extra: readonly string[] = []): boolean {
  if (raw.startsWith("data:") || raw === "about:blank") return true;
  return checkUrl(raw, extra).ok;
}

/** Remove query string and fragment: they often carry tokens and are never needed for a report. */
export function stripQuery(raw: string): string {
  const cut = raw.search(/[?#]/);
  return (cut === -1 ? raw : raw.slice(0, cut)).slice(0, 200);
}
