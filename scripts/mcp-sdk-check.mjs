// Third-party client check: drives `kepler mcp serve` with the official MCP TypeScript SDK.
// Usage (from a directory where `@modelcontextprotocol/sdk` is installed):
//   node mcp-sdk-check.mjs <kepler.exe> <git-repo> [<fake-agent.exe>]
// KEPLER_HOME / KEPLER_IPC_NAME should point at a throw-away location. Prints one JSON summary.
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';

const [exe, repo, fakeAgent] = process.argv.slice(2);
const env = { ...process.env, KEPLER_AGENT_BIN: fakeAgent ?? process.env.KEPLER_AGENT_BIN ?? '' };
if (!env.KEPLER_AGENT_BIN) delete env.KEPLER_AGENT_BIN;
const transport = new StdioClientTransport({ command: exe, args: ['mcp', 'serve'], env, stderr: 'ignore' });
const client = new Client({ name: 'kepler-sdk-check', version: '0' });
const out = { steps: [] };
const ok = (name, cond, detail = '') => {
  out.steps.push({ name, pass: !!cond, detail: String(detail).slice(0, 200) });
  if (!cond) process.exitCode = 1;
};
const call = async (name, args) => {
  const r = await client.callTool({ name, arguments: args });
  const text = r.content?.[0]?.text ?? '';
  let json;
  try { json = JSON.parse(text); } catch { /* plain text */ }
  return { isError: !!r.isError, text, json };
};

try {
  await client.connect(transport);
  const info = client.getServerVersion();
  ok('initialize', info?.name === 'kepler', JSON.stringify(info));
  const { tools } = await client.listTools();
  ok('tools/list parsed by the SDK', tools.length >= 24, `${tools.length} tools`);
  ok('annotations present', tools.every((t) => typeof t.annotations?.readOnlyHint === 'boolean'));
  const ro = tools.filter((t) => t.annotations.readOnlyHint).length;
  out.readOnlyTools = ro; out.writeTools = tools.length - ro;

  const forge = await call('kepler_forge_analyze', { text: 'arregla el bug del login' });
  ok('forge analyze (vague -> enrich)', !forge.isError && forge.json?.mode === 'enrich', forge.text);
  const g1 = await call('kepler_guardian_check', { command: 'rm -rf /' });
  ok('guardian: rm -rf / -> ask', g1.json?.verdict === 'ask', g1.text);
  const g2 = await call('kepler_guardian_check', { path: '.env' });
  ok('guardian: .env -> deny', g2.json?.verdict === 'deny', g2.text);
  const cyc = await call('kepler_orbit_plan', { plan: { name: 'loop', tasks: [
    { id: 'a', command: ['git', '--version'], depends_on: ['b'] },
    { id: 'b', command: ['git', '--version'], depends_on: ['a'] }] } });
  ok('orbit_plan rejects a cycle', cyc.isError && /cycle/.test(cyc.text), cyc.text);
  let bad = false;
  try { await client.callTool({ name: 'kepler_orbit_plan', arguments: { plan: { name: 'x', tasks: [], zzz: 1 } } }); } catch { bad = true; }
  ok('strict schema: unknown field is a protocol error', bad);

  const sug = await call('kepler_orbit_suggest_plan', {
    objective: 'sdk check',
    subtasks: [{ id: 'a', prompt: 'write a.txt from a' }, { id: 'b', prompt: 'write b.txt from b' }],
    integration_prompt: 'integrate',
  });
  ok('suggest_plan', !sug.isError && sug.json?.validation?.waves?.length === 2, sug.text);

  const plan = { name: 'sdk run', tasks: [
    { id: 'one', command: ['git', '--version'] },
    { id: 'two', command: ['git', 'log', '--oneline'], depends_on: ['one'] }] };
  const t0 = Date.now();
  const run = await call('kepler_orbit_run', { plan, repo });
  ok('orbit_run returns a run id', !run.isError && /^orb-/.test(run.json?.run ?? ''), run.text);
  ok('orbit_run does not block', Date.now() - t0 < 15000, `${Date.now() - t0} ms`);
  let st;
  for (let i = 0; i < 100; i++) {
    st = await call('kepler_orbit_status', { run: run.json.run });
    if (['done', 'failed', 'needs_approval'].includes(st.json?.status)) break;
    await new Promise((r) => setTimeout(r, 300));
  }
  ok('run finished', st.json?.status === 'done', st.text);
  const logs = await call('kepler_orbit_logs', { run: run.json.run, task: 'one' });
  ok('orbit_logs', !logs.isError && /git version/.test(logs.text), logs.text);
  const noConfirm = await call('kepler_orbit_cleanup', { run: run.json.run, confirm: false });
  ok('cleanup refuses without confirm', noConfirm.isError, noConfirm.text);
  const cl = await call('kepler_orbit_cleanup', { run: run.json.run, confirm: true });
  ok('cleanup with confirm', !cl.isError, cl.text);

  if (fakeAgent) {
    const ag = await call('kepler_orbit_run', { repo, plan: sug.json.plan });
    ok('agent run started', !ag.isError, ag.text);
    let s2;
    for (let i = 0; i < 150; i++) {
      s2 = await call('kepler_orbit_status', { run: ag.json.run });
      if (['done', 'failed', 'needs_approval'].includes(s2.json?.status)) break;
      await new Promise((r) => setTimeout(r, 300));
    }
    ok('agent run done (2 agents + integration)', s2.json?.status === 'done', s2.text);
    out.agentRun = ag.json.run;
  }
} catch (e) {
  ok('exception', false, e?.message ?? e);
} finally {
  await client.close().catch(() => {});
}
console.log(JSON.stringify(out, null, 1));
