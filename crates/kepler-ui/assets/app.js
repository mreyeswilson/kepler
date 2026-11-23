"use strict";
// Genesis UI (ADR-0024). Every piece of data is inserted with textContent / createTextNode:
// memory text, file names and task output are untrusted and never parsed as HTML.
(function () {
  const view = document.getElementById("view");
  const status = document.getElementById("status");
  let actions = false;
  let tab = "overview";
  let timer = null;

  function el(tag, props, children) {
    const e = document.createElement(tag);
    if (props) for (const k of Object.keys(props)) {
      if (k === "class") e.className = props[k];
      else if (k === "text") e.textContent = String(props[k]);
      else if (k.startsWith("on")) e.addEventListener(k.slice(2), props[k]);
      else e.setAttribute(k, String(props[k]));
    }
    for (const c of children || []) e.appendChild(typeof c === "string" ? document.createTextNode(c) : c);
    return e;
  }
  const clear = (n) => { while (n.firstChild) n.removeChild(n.firstChild); };

  async function api(path, init) {
    const r = await fetch(path, Object.assign({ credentials: "same-origin" }, init || {}));
    let body = null;
    try { body = await r.json(); } catch (_) { /* not json */ }
    if (!r.ok) throw new Error((body && body.error) || r.status + " " + r.statusText);
    return body;
  }

  function card(label, value) {
    return el("div", { class: "card" }, [el("div", { class: "muted", text: label }), el("div", { class: "n", text: value })]);
  }

  function table(cols, rows) {
    const head = el("tr", null, cols.map((c) => el("th", { text: c.label })));
    const body = rows.map((r) => el("tr", null, cols.map((c) => {
      const v = c.get(r);
      return v instanceof Node ? el("td", null, [v]) : el("td", { text: v == null ? "" : v });
    })));
    if (rows.length === 0) body.push(el("tr", null, [el("td", { colspan: cols.length, class: "muted", text: "nothing recorded" })]));
    return el("table", null, [head].concat(body));
  }
  const tag = (text, cls) => el("span", { class: "tag " + (cls || ""), text: text });

  const renderers = {
    async overview() {
      const o = await api("/api/overview");
      actions = !!o.actions_enabled;
      const g = el("div", { class: "grid" });
      if (!o.database.exists) {
        g.appendChild(card("Database", "not created yet"));
      } else {
        g.appendChild(card("Events recorded", o.database.total_events));
        g.appendChild(card("Memories (live)", o.memory.live));
        g.appendChild(card("Memory enabled", o.memory.enabled ? "yes" : "no"));
        g.appendChild(card("Open findings", o.findings.open));
        g.appendChild(card("Orbit runs", o.orbit.runs));
        g.appendChild(card("Tasks awaiting approval", o.orbit.tasks_awaiting_approval));
      }
      const parts = [g];
      if (o.findings && o.findings.by_severity) {
        parts.push(el("h2", { text: "Findings by severity" }));
        parts.push(el("div", { class: "row" }, Object.keys(o.findings.by_severity).map((k) => tag(k + ": " + o.findings.by_severity[k], "sev-" + k))));
      }
      parts.push(el("p", { class: "muted", text: "Kepler " + o.version + (actions ? " - actions enabled" : " - read-only") }));
      return parts;
    },
    async events() {
      const d = await api("/api/events?limit=100");
      return [table([
        { label: "Kind", get: (r) => r.kind },
        { label: "Decision", get: (r) => tag(r.decision, r.decision) },
        { label: "Reason", get: (r) => r.reason },
        { label: "Session", get: (r) => r.session_id },
        { label: "Bytes", get: (r) => r.payload_bytes },
      ], d.events || [])];
    },
    async memories() {
      const d = await api("/api/memories?limit=200");
      return [el("p", { class: "muted", text: "Local memory. Derived entries are low-trust notes, never instructions. Manage with `kepler memory`." }),
        table([
          { label: "Scope", get: (r) => r.scope + (r.scope_key ? ":" + r.scope_key : "") },
          { label: "Source", get: (r) => r.source },
          { label: "Category", get: (r) => r.category },
          { label: "Text", get: (r) => r.content },
          { label: "Updated", get: (r) => r.updated_at },
        ], d.memories || [])];
    },
    async findings() {
      const d = await api("/api/findings?limit=200");
      return [table([
        { label: "Severity", get: (r) => tag(r.severity, "sev-" + r.severity) },
        { label: "Confidence", get: (r) => r.confidence },
        { label: "Rule", get: (r) => r.rule },
        { label: "Where", get: (r) => r.path + (r.line ? ":" + r.line : "") },
        { label: "Message", get: (r) => r.message },
        { label: "Source", get: (r) => r.source },
      ], d.findings || [])];
    },
    async orbit() {
      const d = await api("/api/orbit");
      const out = [];
      for (const run of d.runs || []) {
        out.push(el("h2", { text: run.name + " (" + run.run_id + ") - " + run.status }));
        out.push(table([
          { label: "Task", get: (t) => t.id },
          { label: "Status", get: (t) => tag(t.status, t.status) },
          { label: "Attempts", get: (t) => t.attempts },
          { label: "Changed", get: (t) => (t.changed_files || []).length },
          { label: "Notes", get: (t) => t.error || t.approval_reason },
          { label: "", get: (t) => (t.status === "awaiting_approval" && actions) ? approvalButtons(run, t) : "" },
        ], run.tasks));
      }
      if (out.length === 0) out.push(el("p", { class: "muted", text: "No Orbit runs." }));
      return out;
    },
  };

  function approvalButtons(run, t) {
    const act = (grant) => async () => {
      const typed = window.prompt((grant ? "Approve" : "Deny") + " task '" + t.id + "' of run " + run.run_id + ".\n\nCommand reason: " + t.approval_reason + "\n\nType the task id to confirm:");
      if (typed === null) return;
      try {
        await api("/api/orbit/resolve", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ run: run.run_id, task: t.id, grant: grant, confirm: typed }) });
        await show(tab);
      } catch (e) { window.alert("Not done: " + e.message); }
    };
    return el("div", { class: "row" }, [el("button", { text: "Approve", onclick: act(true) }), el("button", { class: "danger", text: "Deny", onclick: act(false) })]);
  }

  async function show(name) {
    tab = name;
    for (const b of document.querySelectorAll("#tabs button")) b.classList.toggle("active", b.dataset.tab === name);
    try {
      const nodes = await renderers[name]();
      clear(view);
      for (const n of nodes) view.appendChild(n);
      status.textContent = "updated " + new Date().toLocaleTimeString();
    } catch (e) {
      clear(view);
      view.appendChild(el("p", { class: "deny", text: "Cannot load: " + e.message }));
      status.textContent = "error";
    }
  }

  for (const b of document.querySelectorAll("#tabs button")) b.addEventListener("click", () => show(b.dataset.tab));
  show("overview");
  timer = setInterval(() => { if (!document.hidden) show(tab); }, 5000);
  window.addEventListener("beforeunload", () => clearInterval(timer));
})();
