#!/usr/bin/env node
/**
 * Entry point: `node dist/src/main.js`. Started on demand by `kepler vision`; never at boot.
 * Environment: KEPLER_VISION_ALLOW_HOSTS (extra hosts), KEPLER_VISION_MAX_RSS_MB (default 768),
 * KEPLER_VISION_IDLE_MS (default 60000).
 */
import { parseAllowedHosts } from "./allowlist.ts";
import { createPlaywrightDriver } from "./driver.ts";
import { runWorker } from "./worker.ts";

function intEnv(name: string, fallback: number): number {
  const n = Number.parseInt(process.env[name] ?? "", 10);
  return Number.isFinite(n) && n >= 0 ? n : fallback;
}

const allowedHosts = parseAllowedHosts(process.env["KEPLER_VISION_ALLOW_HOSTS"]);
await runWorker({
  input: process.stdin,
  output: process.stdout,
  driver: createPlaywrightDriver(allowedHosts),
  allowedHosts,
  rssLimitBytes: intEnv("KEPLER_VISION_MAX_RSS_MB", 768) * 1024 * 1024,
  idleMs: intEnv("KEPLER_VISION_IDLE_MS", 60_000),
});
process.exit(0);
