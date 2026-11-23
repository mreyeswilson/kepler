import { tmpdir } from "node:os";
import { join } from "node:path";

/** Mirrors `kepler-ipc` endpoint naming (ADR-0007). Naming rules are part of the wire protocol. */
export const DEFAULT_NAME = "daemon";
export const ENV_NAME = "KEPLER_IPC_NAME";

type Env = Readonly<Record<string, string | undefined>>;

export function isValidName(name: string): boolean {
  return /^[a-z0-9_-]{1,32}$/.test(name);
}

/** `KEPLER_IPC_NAME` if set and valid, else `daemon`. */
export function endpointName(env: Env = process.env): string {
  const raw = env[ENV_NAME];
  return raw !== undefined && isValidName(raw) ? raw : DEFAULT_NAME;
}

/** Sanitized OS user name (first non-empty of USERNAME, USER, LOGNAME), max 32 code points. */
export function userTag(env: Env = process.env): string {
  let raw = "user";
  for (const key of ["USERNAME", "USER", "LOGNAME"]) {
    const v = env[key];
    if (v !== undefined && v !== "") {
      raw = v;
      break;
    }
  }
  return Array.from(raw)
    .map((c) => (/^[A-Za-z0-9_-]$/.test(c) ? c : "_"))
    .slice(0, 32)
    .join("");
}

export interface EndpointOptions {
  env?: Env;
  platform?: NodeJS.Platform;
  tmp?: string;
}

/** Windows named pipe path. */
export function pipePath(env: Env = process.env): string {
  return String.raw`\\.\pipe\kepler-${userTag(env)}-${endpointName(env)}`;
}

/** Unix socket path: `$XDG_RUNTIME_DIR/kepler/kepler-<name>.sock` or `<tmp>/kepler-<user>/kepler-<name>.sock`. */
export function unixSocketPath(env: Env = process.env, tmp: string = tmpdir()): string {
  const xdg = env["XDG_RUNTIME_DIR"];
  const dir = xdg !== undefined && xdg !== "" ? join(xdg, "kepler") : join(tmp, `kepler-${userTag(env)}`);
  return join(dir, `kepler-${endpointName(env)}.sock`);
}

/** Resolve the per-user daemon endpoint for the current (or given) platform. */
export function resolveEndpoint(opts: EndpointOptions = {}): string {
  const env = opts.env ?? process.env;
  const platform = opts.platform ?? process.platform;
  return platform === "win32" ? pipePath(env) : unixSocketPath(env, opts.tmp);
}
