// Real end-to-end check: a REAL Pi (@earendil-works/pi-coding-agent from npm) loads this package
// through `pi install <path>`, talks to a REAL kepler release daemon over the named pipe/socket, and
// a scripted "faux" model (test-only extension) drives tool calls. No model credentials are needed
// because Pi never calls a remote model: the faux provider answers locally.
//
// Usage:  node adapters/pi/test/e2e/run.mjs
// Env:    KEPLER_BIN         kepler binary (default target/release/kepler[.exe])
//         KEPLER_E2E_PI_DIR  directory holding node_modules with Pi (installed here when missing)
//
// What is NOT covered: interactive TUI approval prompts (print mode has no UI), a real remote model.
import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const adapter = resolve(here, "../..");
const repo = resolve(adapter, "../..");
const win = process.platform === "win32";
const kepler = process.env.KEPLER_BIN ?? join(repo, "target", "release", win ? "kepler.exe" : "kepler");
const piDir = process.env.KEPLER_E2E_PI_DIR ?? join(tmpdir(), "kepler-pi-e2e-install");
const PI_VERSION = process.env.KEPLER_E2E_PI_VERSION ?? "1.6.0";
const npm = win ? "npm.cmd" : "npm";

const results = [];
function check(name, ok, detail = "") {
  results.push({ name, ok });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${ok || !detail ? "" : `\n      ${String(detail).slice(0, 600)}`}`);
}

if (!existsSync(kepler)) {
  console.error(`kepler binary not found: ${kepler} (build with cargo build --release -p kepler-cli)`);
  process.exit(2);
}
const piCli = join(piDir, "node_modules", "@earendil-works", "pi-coding-agent", "dist", "bundle", "cli.js");
if (!existsSync(piCli)) {
  mkdirSync(piDir, { recursive: true });
  if (!existsSync(join(piDir, "package.json"))) writeFileSync(join(piDir, "package.json"), '{"name":"kepler-pi-e2e","private":true}');
  console.log(`installing @earendil-works/pi-coding-agent@${PI_VERSION} into ${piDir} ...`);
  const r = spawnSync(npm, ["install", `@earendil-works/pi-coding-agent@${PI_VERSION}`], { cwd: piDir, stdio: "inherit", shell: win });
  if (r.status !== 0) {
    console.error("npm install failed");
    process.exit(2);
  }
}

const root = mkdtempSync(join(tmpdir(), "kepler-pi-e2e-"));
const keplerHome = join(root, "home");
const agentDir = join(root, "agent");
const proj = join(root, "project");
for (const d of [keplerHome, agentDir, proj]) mkdirSync(d, { recursive: true });
writeFileSync(join(proj, ".env"), "DB_PASSWORD=hunter2-not-a-real-secret\n");
writeFileSync(join(proj, "notes.txt"), "hello\n");
mkdirSync(join(proj, ".git"));
writeFileSync(join(proj, ".git", "HEAD"), "ref: refs/heads/feat/e2e-branch\n");
const ipcName = `pie2e${process.pid}`;
const env = {
  ...process.env,
  KEPLER_HOME: keplerHome,
  KEPLER_IPC_NAME: ipcName,
  KEPLER_PROJECT_ROOTS: proj,
  PI_CODING_AGENT_DIR: agentDir,
  PI_OFFLINE: "1",
  PI_SKIP_VERSION_CHECK: "1",
  PI_TELEMETRY: "0",
};

function k(args) {
  return spawnSync(kepler, args, { env, encoding: "utf8" });
}

function runPi(prompt, steps, tag) {
  const script = join(root, `${tag}.script.json`);
  const log = join(root, `${tag}.model.jsonl`);
  writeFileSync(script, JSON.stringify(steps));
  const r = spawnSync(
    process.execPath,
    [piCli, "-p", prompt, "-e", join(here, "faux-model.ts"), "--provider", "faux", "--model", "scripted", "--no-session"],
    { cwd: proj, env: { ...env, KEPLER_E2E_SCRIPT: script, KEPLER_E2E_LOG: log }, encoding: "utf8", timeout: 120_000 },
  );
  const reqs = existsSync(log)
    ? readFileSync(log, "utf8").trim().split("\n").filter(Boolean).map((l) => JSON.parse(l))
    : [];
  return { code: r.status, stdout: r.stdout ?? "", stderr: r.stderr ?? "", reqs };
}

let daemon;
try {
  // 1. real daemon (the adapter never starts it)
  daemon = spawn(kepler, ["daemon", "--idle-timeout-secs", "0"], { env, stdio: "ignore", windowsHide: true });
  let up = false;
  for (let i = 0; i < 50 && !up; i++) {
    await new Promise((r) => setTimeout(r, 200));
    up = JSON.parse(k(["doctor", "--json"]).stdout || "{}")?.ipc?.daemon?.running === true;
  }
  check("daemon: kepler release binary is serving the IPC endpoint", up);
  if (!up) throw new Error("daemon did not start");

  // 2. install through Pi's own mechanism
  const inst = spawnSync(process.execPath, [piCli, "install", adapter], { env, encoding: "utf8" });
  check("pi install <adapters/pi> succeeds", inst.status === 0, inst.stdout + inst.stderr);
  const list = spawnSync(process.execPath, [piCli, "list"], { env, encoding: "utf8" });
  check("pi list shows the package", /adapters[\\/]pi/.test(list.stdout), list.stdout + list.stderr);

  // 3. harmless tool call: Pi really runs it and Kepler does not interfere
  let r = runPi("list the notes", [{ tool: "bash", args: { command: "echo hello-from-tool" } }, { text: "done" }], "ok");
  check("allowed tool call runs normally", r.code === 0 && r.reqs.at(-1)?.lastToolResult.includes("hello-from-tool"), JSON.stringify(r.reqs));

  // 4. Guardian: reading .env is denied before Pi runs the tool
  r = runPi("show me the config", [{ tool: "bash", args: { command: "cat .env" } }, { text: "done" }], "deny");
  const denied = r.reqs.at(-1)?.lastToolResult ?? "";
  check("secret read (cat .env) is blocked through the real tool_call hook", /Kepler blocked/.test(denied) && !denied.includes("hunter2"), JSON.stringify(r.reqs));

  r = runPi("show me the config", [{ tool: "bash", args: { command: "c\"\"at .e'n'v" } }, { text: "done" }], "deny2");
  const denied2 = r.reqs.at(-1)?.lastToolResult ?? "";
  check("obfuscated secret read (c\"\"at .e'n'v) is blocked", /Kepler blocked/.test(denied2) && !denied2.includes("hunter2"), JSON.stringify(r.reqs));

  // 5. require_approval with no UI (print mode): blocked, never silently allowed
  r = runPi("clean up", [{ tool: "bash", args: { command: "git push --force origin main" } }, { text: "done" }], "appr");
  check("destructive command is blocked when no approval UI exists", /requires approval/.test(r.reqs.at(-1)?.lastToolResult ?? ""), JSON.stringify(r.reqs));

  // 6. secret in a tool result is withheld before it reaches the model
  r = runPi("print a token", [{ tool: "bash", args: { command: "echo ghp_abcdefghijklmnopqrstuvwxyz0123456789" } }, { text: "done" }], "redact");
  const red = r.reqs.at(-1)?.lastToolResult ?? "";
  check("secret in tool output is replaced before the model sees it", /Kepler withheld/.test(red) && !red.includes("ghp_abcdef"), JSON.stringify(r.reqs));

  // 7. memory: explicit directive in one session, recalled in another, labeled and outside the user prompt
  runPi("Remember that the deploy branch is called zeta-9 and we squash merge", [{ text: "noted" }], "mem1");
  await new Promise((r2) => setTimeout(r2, 500));
  r = runPi("which branch do we deploy and how do we merge it", [{ text: "ok" }], "mem2");
  const sys = r.reqs[0]?.systemPrompt ?? "";
  const usr = r.reqs[0]?.lastUser ?? "";
  check("recalled memory reaches the model as a labeled derived block", sys.includes("zeta-9") && sys.includes("NOT user instructions"), sys.slice(-400));
  check("the user's own prompt is untouched", usr === "which branch do we deploy and how do we merge it", usr);

  // 7b. Pi's own write tool -> files.changed -> branch-scoped memory after an ok settlement,
  //     recalled only on the same branch (branch comes from .git/HEAD through the adapter).
  r = runPi("create the zebra module notes", [{ tool: "write", args: { path: "zebra_module.txt", content: "zebra\n" } }, { text: "created" }], "write");
  check("Pi write tool really created the file", existsSync(join(proj, "zebra_module.txt")), JSON.stringify(r.reqs));
  await new Promise((r2) => setTimeout(r2, 500));
  r = runPi("what did the last task on this branch change in the zebra module", [{ text: "ok" }], "branchmem");
  check("branch-scoped memory is recalled on its branch", (r.reqs[0]?.systemPrompt ?? "").includes("zebra_module.txt"), (r.reqs[0]?.systemPrompt ?? "").slice(-300));
  writeFileSync(join(proj, ".git", "HEAD"), "ref: refs/heads/other-branch\n");
  r = runPi("what did the last task on this branch change in the zebra module", [{ text: "ok" }], "branchmem2");
  check("...and not on another branch", !(r.reqs[0]?.systemPrompt ?? "").includes("zebra_module.txt"), (r.reqs[0]?.systemPrompt ?? "").slice(-300));
  writeFileSync(join(proj, ".git", "HEAD"), "ref: refs/heads/feat/e2e-branch\n");

  // 7c. negative control: the same secret read WITHOUT Kepler installed is not blocked
  const bare = mkdtempSync(join(tmpdir(), "kepler-pi-bare-"));
  const ctl = spawnSync(
    process.execPath,
    [piCli, "-p", "show me the config", "-e", join(here, "faux-model.ts"), "--provider", "faux", "--model", "scripted", "--no-session"],
    {
      cwd: proj,
      env: { ...env, PI_CODING_AGENT_DIR: bare, KEPLER_E2E_SCRIPT: join(root, "deny.script.json"), KEPLER_E2E_LOG: join(root, "ctl.jsonl") },
      encoding: "utf8",
      timeout: 120_000,
    },
  );
  const ctlLog = existsSync(join(root, "ctl.jsonl")) ? readFileSync(join(root, "ctl.jsonl"), "utf8") : "";
  check("control: without the adapter the same call leaks the secret (test is not vacuous)", ctlLog.includes("hunter2"), ctl.stderr + ctlLog);
  rmSync(bare, { recursive: true, force: true });

  // 8. what the core recorded
  await new Promise((r2) => setTimeout(r2, 500));
  const insp = JSON.parse(k(["inspect", "--limit", "300"]).stdout);
  const kinds = new Set(insp.recent_events.map((e) => e.kind));
  for (const kind of ["input.received", "agent.started", "tool.call.requested", "tool.call.completed", "agent.settled", "session.closed", "context.prepared", "files.changed"]) {
    check(`core received ${kind} from the real Pi`, kinds.has(kind), [...kinds].join(","));
  }
  const decisions = insp.recent_events.filter((e) => e.kind === "tool.call.requested").map((e) => e.decision);
  check("core recorded deny and require_approval decisions", decisions.includes("deny") && decisions.includes("require_approval"), decisions.join(","));
  const dbBytes = readFileSync(insp.database.path);
  check("database holds neither the .env value nor the token", !dbBytes.includes("hunter2") && !dbBytes.includes("ghp_abcdefghijkl"));
} catch (e) {
  check("e2e run completed without exceptions", false, e?.stack ?? String(e));
} finally {
  if (daemon) {
    if (win) spawnSync("taskkill", ["/PID", String(daemon.pid), "/T", "/F"], { stdio: "ignore" });
    else daemon.kill();
  }
  await new Promise((r) => setTimeout(r, 300));
  try {
    rmSync(root, { recursive: true, force: true });
  } catch {
    /* best effort */
  }
}
const failed = results.filter((r) => !r.ok).length;
console.log(`\n${results.length - failed}/${results.length} checks passed`);
process.exit(failed === 0 ? 0 : 1);
