"use strict";
// Kepler UI (ADR-0024). Every piece of data is inserted with textContent / createTextNode:
// session ids, file names, memory text and task output are untrusted and never parsed as HTML.
(function () {
  const view = document.getElementById("view");
  const statusEl = document.getElementById("status");
  const dot = document.getElementById("dot");
  const healthText = document.getElementById("healthtext");
  const warningsEl = document.getElementById("warnings");
  const POLL_MS = 5000;
  let actions = false;
  let current = { name: "resumen", arg: "" };
  const evFilter = { kind: "", decision: "", session: "", window: "", module: "", offset: 0 };
  let sessOffset = 0;

  const STATE = {
    active: "Activo en Claude Code",
    seen: "Visto (no reciente)",
    manual: "Solo manual (CLI)",
    none: "Sin actividad",
    not_implemented: "No implementado",
  };

  function el(tag, props, children) {
    const e = document.createElement(tag);
    if (props) for (const k of Object.keys(props)) {
      if (props[k] == null) continue;
      if (k === "class") e.className = props[k];
      else if (k === "text") e.textContent = String(props[k]);
      else if (k === "width") e.style.width = props[k];
      else if (k.startsWith("on")) e.addEventListener(k.slice(2), props[k]);
      else e.setAttribute(k, String(props[k]));
    }
    for (const c of children || []) if (c != null) e.appendChild(typeof c === "string" ? document.createTextNode(c) : c);
    return e;
  }
  const clear = (n) => { while (n.firstChild) n.removeChild(n.firstChild); };
  const tag = (text, cls) => el("span", { class: "tag " + (cls || ""), text: text });
  const h2 = (t) => el("h2", { text: t });
  const note = (t) => el("p", { class: "muted", text: t });
  const fmtNum = (n) => (n == null ? "-" : Number(n).toLocaleString());
  const fmtBytes = (n) => (n == null ? "-" : n < 1024 ? n + " B" : n < 1048576 ? (n / 1024).toFixed(1) + " KB" : (n / 1048576).toFixed(1) + " MB");
  const fmtTime = (s) => { if (!s) return "-"; const d = new Date(s); return isNaN(d) ? String(s) : d.toLocaleString(); };
  const ago = (s) => {
    if (!s) return "nunca";
    const d = new Date(s); if (isNaN(d)) return String(s);
    const sec = Math.max(0, Math.round((Date.now() - d.getTime()) / 1000));
    if (sec < 60) return "hace " + sec + " s";
    if (sec < 3600) return "hace " + Math.round(sec / 60) + " min";
    if (sec < 86400) return "hace " + Math.round(sec / 3600) + " h";
    return "hace " + Math.round(sec / 86400) + " d";
  };

  async function api(path, init) {
    const r = await fetch(path, Object.assign({ credentials: "same-origin" }, init || {}));
    let body = null;
    try { body = await r.json(); } catch (_) { /* not json */ }
    if (!r.ok) throw new Error((body && body.error) || r.status + " " + r.statusText);
    return body;
  }
  const qs = (o) => Object.keys(o).filter((k) => o[k] !== "" && o[k] != null).map((k) => encodeURIComponent(k) + "=" + encodeURIComponent(o[k])).join("&");

  function card(label, value, sub) {
    return el("div", { class: "card" }, [el("div", { class: "muted", text: label }), el("div", { class: "n", text: value }), sub ? el("div", { class: "s", text: sub }) : null]);
  }

  function table(cols, rows, opts) {
    const head = el("tr", null, cols.map((c) => el("th", { text: c.label })));
    const body = rows.map((r) => {
      const props = {};
      if (opts && opts.onrow) { props.class = "click"; props.onclick = () => opts.onrow(r); }
      return el("tr", props, cols.map((c) => {
        const v = c.get(r);
        return v instanceof Node ? el("td", null, [v]) : el("td", { text: v == null ? "" : v });
      }));
    });
    if (rows.length === 0) body.push(el("tr", null, [el("td", { colspan: cols.length, class: "muted", text: (opts && opts.empty) || "sin datos" })]));
    return el("div", { class: "tablewrap" }, [el("table", { class: opts && opts.nw ? "nw" : null }, [head].concat(body))]);
  }

  // Distribution as labelled bars (counts, not percentages of anything invented).
  function bars(map, empty) {
    const keys = Object.keys(map || {});
    if (keys.length === 0) return note(empty || "sin datos");
    const max = Math.max.apply(null, keys.map((k) => map[k])) || 1;
    return el("div", { class: "bars" }, keys.sort((a, b) => map[b] - map[a]).map((k) => el("div", { class: "bar" }, [
      el("span", { text: k }),
      el("div", { class: "track" }, [el("div", { class: "fill", width: Math.max(2, Math.round(100 * map[k] / max)) + "%" })]),
      el("span", { class: "v", text: fmtNum(map[k]) }),
    ])));
  }
  const listToMap = (l) => { const m = {}; for (const x of l || []) m[x.name] = x.count; return m; };

  function pager(offset, limit, total, onGo) {
    const to = Math.min(offset + limit, total);
    return el("div", { class: "pager" }, [
      el("button", { text: "Anterior", disabled: offset <= 0 ? "disabled" : null, onclick: () => onGo(Math.max(0, offset - limit)) }),
      el("span", { class: "muted", text: total === 0 ? "0" : (offset + 1) + "-" + to + " de " + fmtNum(total) }),
      el("button", { text: "Siguiente", disabled: offset + limit >= total ? "disabled" : null, onclick: () => onGo(offset + limit) }),
    ]);
  }

  function eventCols(withSession) {
    const cols = [
      { label: "Cuando", get: (r) => fmtTime(r.at) },
      { label: "Tipo", get: (r) => r.kind },
      { label: "Decision", get: (r) => tag(r.decision, r.decision) },
      { label: "Motivo", get: (r) => r.reason },
      { label: "Bytes", get: (r) => r.payload_bytes },
      { label: "Modulo", get: (r) => r.actors },
      { label: "ms", get: (r) => r.ms },
      { label: "Detalle", get: (r) => (r.detail ? el("span", { class: "mono wrapall", text: r.detail }) : "") },
    ];
    if (withSession) cols.splice(1, 0, { label: "Sesion", get: (r) => el("a", { href: "#/sesiones/" + encodeURIComponent(r.session_id), text: r.session }) });
    return cols;
  }

  const renderers = {
    async resumen() {
      const o = await api("/api/summary");
      const d = o.daemon || {}, db = o.database || {}, ev = o.events || {};
      const dstate = { running: "en ejecucion", stopped: "detenido", unresponsive: "no responde", unknown: "desconocido" }[d.state] || d.state;
      const g = el("div", { class: "grid" }, [
        card("Kepler (UI)", "v" + o.version, o.actions_enabled ? "acciones habilitadas" : "solo lectura"),
        card("Identidad (SO)", (o.identity.user || "?") + "@" + (o.identity.host || "?"), o.identity.os + " / " + o.identity.arch),
        card("Daemon", dstate, d.version ? "version " + d.version : (d.endpoint || "")),
        card("Uptime del daemon", d.uptime_secs == null ? "no disponible" : d.uptime_secs + " s", d.note || ""),
        card("Base de datos", db.exists ? (db.error ? "con error" : "ok") : "no creada", db.exists ? fmtBytes(db.size_bytes) + " - esquema " + db.schema_version + "/" + db.latest_schema : ""),
        card("Eventos totales", fmtNum(ev.total), fmtNum(ev.last_minute) + " en el ultimo minuto"),
        card("Eventos por minuto", Number(ev.per_minute_10m || 0).toFixed(1), "media de los ultimos 10 min"),
      ]);
      const parts = [g];
      parts.push(h2("Rutas"));
      parts.push(el("p", { class: "mono", text: "base: " + (db.path || "-") + (db.journal_mode ? "  (journal " + db.journal_mode + ")" : "") }));
      if (d.endpoint) parts.push(el("p", { class: "mono", text: "endpoint del daemon: " + d.endpoint }));
      parts.push(h2("Harnesses vistos"));
      parts.push(table([
        { label: "Harness", get: (r) => r.harness },
        { label: "Eventos", get: (r) => fmtNum(r.events) },
        { label: "Sesiones", get: (r) => fmtNum(r.sessions) },
        { label: "Ultimo evento", get: (r) => ago(r.last_event) },
      ], o.harnesses || [], { empty: "ningun harness ha enviado eventos todavia" }));
      return parts;
    },

    async modulos() {
      const d = await api("/api/modules");
      const rows = d.modules || [];
      const t = table([
        { label: "Modulo", get: (m) => el("div", { class: "modcard" }, [el("strong", { text: m.name }), el("span", { class: "muted", text: "Implementado: " + m.implemented })]) },
        { label: "Que hace (segun docs)", get: (m) => el("div", null, [el("div", { text: m.what }), el("div", { class: "limit", text: "Limite: " + m.limitation })]) },
        { label: "En el flujo de Claude Code", get: (m) => el("div", null, [tag(STATE[m.state] || m.state, "st-" + m.state), el("div", { class: "s muted", text: m.evidence })]) },
        { label: "Ultima actividad", get: (m) => el("div", null, [el("div", { text: m.last_activity ? ago(m.last_activity) : "sin actividad" }), m.last_activity_claude_code ? el("div", { class: "s muted", text: "Claude Code: " + ago(m.last_activity_claude_code) }) : null]) },
        { label: "Contadores", get: (m) => el("div", null, (m.counters || []).map((c) => el("div", null, [el("strong", { text: fmtNum(c.value) }), " ", el("span", { class: "muted", text: c.label })]))) },
      ], rows);
      const leg = Object.keys(d.legend || {}).map((k) => el("div", null, [tag(STATE[k] || k, "st-" + k), " " + d.legend[k]]));
      return [note(d.note), t, h2("Leyenda"), el("div", null, leg)];
    },

    async sesiones(arg) {
      if (arg) return sessionDetail(arg);
      const d = await api("/api/sessions?" + qs({ limit: 30, offset: sessOffset }));
      return [
        note("El proyecto se muestra como identificador hash (no la ruta). Selecciona una sesion para ver su linea de tiempo."),
        table([
          { label: "Sesion", get: (r) => r.session },
          { label: "Harness", get: (r) => r.harness },
          { label: "Proyecto", get: (r) => r.project },
          { label: "Primer evento", get: (r) => fmtTime(r.first_seen) },
          { label: "Ultimo evento", get: (r) => ago(r.last_seen) },
          { label: "Prompts", get: (r) => r.prompts },
          { label: "Eventos", get: (r) => r.events },
          { label: "Decisiones", get: (r) => el("div", null, Object.keys(r.decisions).map((k) => tag(k + ": " + r.decisions[k], k))) },
        ], d.sessions || [], { nw: true, onrow: (r) => { location.hash = "#/sesiones/" + encodeURIComponent(r.session_id); }, empty: "no hay sesiones registradas" }),
        pager(d.offset, d.limit, d.total, (o) => { sessOffset = o; show(); }),
      ];
    },

    async eventos() {
      const f = evFilter;
      const d = await api("/api/events?" + qs({ kind: f.kind, decision: f.decision, session: f.session, window: f.window, module: f.module, limit: 50, offset: f.offset }));
      const input = (key, ph) => el("input", { placeholder: ph, value: f[key], size: 14, onchange: (e) => { f[key] = e.target.value.trim(); f.offset = 0; show(); } });
      const select = (key, opts) => el("select", { onchange: (e) => { f[key] = e.target.value; f.offset = 0; show(); } }, opts.map((o) => { const x = el("option", { value: o[0], text: o[1] }); if (f[key] === o[0]) x.selected = true; return x; }));
      return [
        el("div", { class: "row" }, [
          input("kind", "tipo (p.ej. tool.call.requested)"),
          select("decision", [["", "toda decision"], ["passthrough", "passthrough"], ["enrich", "enrich"], ["require_approval", "require_approval"], ["deny", "deny"], ["redact", "redact"]]),
          select("module", [["", "todo modulo"], ["forge", "forge"], ["memory", "memory"], ["guardian", "guardian"], ["focus", "focus"], ["sentinel", "sentinel"]]),
          input("session", "id de sesion"),
          select("window", [["", "todo el tiempo"], ["5m", "ultimos 5 min"], ["15m", "ultimos 15 min"], ["1h", "ultima hora"], ["24h", "ultimas 24 h"], ["7d", "ultimos 7 dias"]]),
          el("button", { text: "Limpiar", onclick: () => { for (const k of Object.keys(f)) f[k] = k === "offset" ? 0 : ""; show(); } }),
        ]),
        d.attribution === false ? el("div", { class: "banner", text: "Esquema antiguo: sin columnas de modulo/ms hasta que el daemon migre." }) : null,
        table(eventCols(true), d.events || [], { nw: true, empty: "ningun evento coincide" }),
        pager(d.offset, d.limit, d.total, (o) => { f.offset = o; show(); }),
        note(d.note),
      ];
    },

    async forge() {
      const d = await api("/api/forge");
      const out = [];
      if (d.database === false) return [note("Sin base de datos todavia.")];
      if (d.attribution === false) out.push(el("div", { class: "banner", text: d.note }));
      else {
        out.push(h2("Modos (" + fmtNum(d.classified_total) + " entradas clasificadas)"));
        out.push(bars(d.modes, "ninguna entrada clasificada todavia"));
        out.push(h2("Clase por reglas"));
        out.push(bars(d.classes));
        out.push(h2("Motivo de la regla"));
        out.push(bars(d.rules));
        out.push(h2("Confianza de las reglas"));
        out.push(bars(d.confidence));
        out.push(h2("Ultimas clasificaciones (solo metadatos)"));
        out.push(table([
          { label: "Cuando", get: (r) => ago(r.at) },
          { label: "Sesion", get: (r) => r.session },
          { label: "Modo", get: (r) => tag(r.mode || "?") },
          { label: "Regla", get: (r) => r.rule },
          { label: "Clase", get: (r) => r.class },
          { label: "Confianza", get: (r) => r.confidence },
          { label: "Bytes", get: (r) => r.bytes },
        ], d.recent || [], { empty: "sin clasificaciones" }));
        out.push(note(d.note));
      }
      const m = d.model || {};
      out.push(h2("Modelo opcional (claude -p)"));
      if (!m.available) out.push(note(m.note || "sin datos del modelo"));
      else out.push(el("div", { class: "grid" }, [
        card("Llamadas", fmtNum(m.calls), fmtNum(m.ok) + " ok / " + fmtNum(m.failed) + " fallos"),
        card("Cache", fmtNum(m.cache_hits) + " aciertos", fmtNum(m.cache_entries) + " entradas"),
        card("Latencia media", m.latency_avg_ms == null ? "-" : m.latency_avg_ms + " ms", "p95: " + (m.latency_p95_ms == null ? "-" : m.latency_p95_ms + " ms") + " (" + m.latency_samples + " muestras)"),
        card("Tokens", fmtNum(m.tokens_in) + " / " + fmtNum(m.tokens_out), "entrada / salida (informado por la CLI)"),
        card("Coste", "$" + Number(m.cost_usd).toFixed(4), "informado por la CLI"),
        card("Circuit breaker", m.breaker, m.breaker === "abierto" ? m.breaker_seconds_left + " s restantes" : (m.last_error ? "ultimo error: " + m.last_error : "")),
      ]));
      return out;
    },

    async guardian() {
      const d = await api("/api/guardian");
      if (d.database === false) return [note("Sin base de datos todavia.")];
      const out = [];
      out.push(h2("Decisiones sobre herramientas"));
      out.push(bars(listToMap(d.tool_decisions), "sin eventos de herramientas"));
      out.push(h2("Motivos de asks / denies / redacciones"));
      out.push(table([
        { label: "Decision", get: (r) => tag(r.decision, r.decision) },
        { label: "Motivo", get: (r) => r.reason },
        { label: "Veces", get: (r) => fmtNum(r.count) },
      ], d.reasons || [], { empty: "Guardian no ha bloqueado, pedido aprobacion ni redactado nada" }));
      out.push(h2("Herramientas mas usadas"));
      out.push(d.tools_available ? table([
        { label: "Herramienta", get: (r) => r.tool },
        { label: "Eventos", get: (r) => fmtNum(r.events) },
        { label: "Marcados", get: (r) => fmtNum(r.flagged) },
      ], d.top_tools || [], { empty: "sin datos de herramientas" }) : note("Esquema antiguo: el nombre de la herramienta no se guardaba."));
      out.push(h2("Tokens: medidos vs estimados (nunca se suman)"));
      out.push(table([
        { label: "Tipo", get: (r) => r.kind },
        { label: "Base", get: (r) => tag(r.basis) },
        { label: "Muestras", get: (r) => fmtNum(r.samples) },
        { label: "Tokens", get: (r) => fmtNum(r.tokens) },
        { label: "Items", get: (r) => fmtNum(r.items) },
      ], d.token_metrics || [], { empty: "sin metricas de tokens" }));
      out.push(h2("Presupuestos"));
      out.push(el("div", { class: "grid" }, [
        card("Contexto derivado", fmtNum(d.budgets.context_tokens) + " tokens", "estimados"),
        card("Memory", fmtNum(d.budgets.memory_tokens) + " tokens", "estimados"),
      ]));
      out.push(note(d.budgets.note));
      out.push(el("p", { class: "limit", text: d.note }));
      return out;
    },

    async memoria() {
      const [d, list] = await Promise.all([api("/api/memory"), api("/api/memories?limit=100")]);
      if (d.database === false) return [note("Sin base de datos todavia.")];
      const out = [
        el("div", { class: "grid" }, [
          card("Total", fmtNum(d.total)), card("Vivas", fmtNum(d.live)), card("Reemplazadas", fmtNum(d.superseded)),
          card("Del usuario", fmtNum(d.user_sourced)),
          card("Estado", d.enabled && !d.force_disabled_env ? "habilitada" : "deshabilitada", d.force_disabled_env ? "KEPLER_MEMORY=off" : ""),
          card("Retencion", d.retention_days ? d.retention_days + " dias" : "sin limite", "max " + fmtNum(d.max_live_per_project) + " vivas por proyecto"),
          card("Inyecciones en eventos", d.recalls_in_events ? fmtNum(d.recalls_in_events.count) : "n/d", d.recalls_in_events ? "ultima " + ago(d.recalls_in_events.last) : "esquema antiguo"),
        ]),
        h2("Por ambito"),
        table([{ label: "Ambito", get: (r) => r.scope }, { label: "Total", get: (r) => r.total }, { label: "Vivas", get: (r) => r.live }], d.by_scope || [], { empty: "sin memorias" }),
        h2("Por proyecto"),
        table([{ label: "Proyecto", get: (r) => r.project }, { label: "Total", get: (r) => r.total }, { label: "Vivas", get: (r) => r.live }, { label: "Usos", get: (r) => r.uses }, { label: "Ultimo uso", get: (r) => ago(r.last_used) }], d.by_project || [], { empty: "sin memorias" }),
        h2("Ultimos aciertos (metadatos)"),
        table([{ label: "#", get: (r) => r.id }, { label: "Ambito", get: (r) => r.scope }, { label: "Categoria", get: (r) => r.category }, { label: "Origen", get: (r) => r.source }, { label: "Confianza", get: (r) => r.trust }, { label: "Usos", get: (r) => r.use_count }, { label: "Ultimo uso", get: (r) => ago(r.last_used) }], d.recent_uses || [], { empty: "ninguna memoria ha sido recuperada todavia" }),
        h2("Memorias guardadas (texto redactado, solo las tuyas)"),
        note("Las memorias derivadas son notas de baja confianza, nunca instrucciones. Administra con `kepler memory`."),
        table([
          { label: "Ambito", get: (r) => r.scope + (r.scope_key ? ":" + r.scope_key : "") },
          { label: "Origen", get: (r) => r.source },
          { label: "Categoria", get: (r) => r.category },
          { label: "Texto", get: (r) => r.content },
          { label: "Actualizada", get: (r) => r.updated_at },
        ], list.memories || [], { empty: "sin memorias" }),
      ];
      return out;
    },

    async sentinel() {
      const d = await api("/api/sentinel");
      if (d.database === false) return [note("Sin base de datos todavia.")];
      const fc = d.files_changed_events || {};
      return [
        el("div", { class: "grid" }, [
          card("Eventos files.changed", fmtNum(fc.count), "Claude Code (7 d): " + fmtNum(fc.claude_code_7d) + " - ultimo " + ago(fc.last)),
          card("Hallazgos abiertos", fmtNum(Object.values(listToMap(d.by_severity)).reduce((a, b) => a + b, 0))),
        ]),
        h2("Por severidad"), bars(listToMap(d.by_severity), "sin hallazgos abiertos"),
        h2("Por regla"), bars(listToMap(d.by_rule), "sin hallazgos"),
        h2("Por archivo"), bars(listToMap(d.by_file), "sin hallazgos"),
        h2("Estado"), bars(listToMap(d.by_status), "sin hallazgos"),
        h2("Ultimos hallazgos"),
        table([
          { label: "Cuando", get: (r) => ago(r.at) },
          { label: "Severidad", get: (r) => tag(r.severity, "sev-" + r.severity) },
          { label: "Confianza", get: (r) => r.confidence },
          { label: "Regla", get: (r) => r.rule },
          { label: "Donde", get: (r) => r.path + (r.line ? ":" + r.line : "") },
          { label: "Estado", get: (r) => r.status },
        ], d.latest || [], { empty: "Sentinel no ha registrado hallazgos" }),
        el("p", { class: "muted", text: d.note }),
      ];
    },

    async orbit() {
      const d = await api("/api/orbit");
      const out = [el("p", { class: "limit", text: "Orbit es hoy un ejecutor de tareas por CLI (`kepler orbit`); no se invoca desde el flujo de Claude Code." })];
      let waiting = 0;
      for (const run of d.runs || []) waiting += run.tasks.filter((t) => t.status === "awaiting_approval").length;
      out.push(el("div", { class: "grid" }, [card("Runs", fmtNum((d.runs || []).length)), card("Aprobaciones pendientes", fmtNum(waiting), actions ? "acciones habilitadas" : "solo lectura (--allow-actions para aprobar)")]));
      for (const run of d.runs || []) {
        out.push(h2(run.name + " (" + run.run_id + ") - " + run.status));
        out.push(el("p", { class: "muted", text: "creado " + fmtTime(run.created_at) + " - paralelismo maximo " + run.max_parallel }));
        out.push(table([
          { label: "Tarea", get: (t) => t.id },
          { label: "Estado", get: (t) => tag(t.status, t.status) },
          { label: "Depende de", get: (t) => (t.depends_on || []).join(", ") },
          { label: "Rama", get: (t) => t.branch },
          { label: "Intentos", get: (t) => t.attempts },
          { label: "Archivos", get: (t) => (t.changed_files || []).length },
          { label: "Notas", get: (t) => t.error || t.approval_reason },
          { label: "", get: (t) => (t.status === "awaiting_approval" && actions) ? approvalButtons(run, t) : "" },
        ], run.tasks));
      }
      if ((d.runs || []).length === 0) out.push(note("No hay runs de Orbit."));
      return out;
    },

    async vision() {
      const d = await api("/api/vision-focus");
      const v = d.vision, f = d.focus;
      return [
        h2("Vision"),
        el("div", { class: "grid" }, [
          card("Worker", v.worker_installed ? "instalado" : "no instalado", v.worker_via_env ? "via KEPLER_VISION_WORKER" : "kepler vision setup"),
          card("Edge", v.browser.edge_found ? "detectado" : "no detectado"),
          card("Chrome", v.browser.chrome_found ? "detectado" : "no detectado"),
          card("Hallazgos abiertos", fmtNum(v.findings_open)),
        ]),
        note(v.browser.note),
        h2("Ultimos checks"),
        table([
          { label: "Cuando", get: (r) => ago(r.at) },
          { label: "Severidad", get: (r) => tag(r.severity, "sev-" + r.severity) },
          { label: "Regla", get: (r) => r.rule },
          { label: "Objetivo", get: (r) => r.target },
          { label: "Estado", get: (r) => r.status },
        ], v.findings, { empty: "Vision no ha registrado checks" }),
        h2("Focus: avisos por sesion"),
        f.attribution === false ? el("div", { class: "banner", text: "Esquema antiguo: no se registra quien emitio cada aviso." }) : null,
        table([
          { label: "Sesion", get: (r) => el("a", { href: "#/sesiones/" + encodeURIComponent(r.session_id), text: r.session }) },
          { label: "Avisos", get: (r) => r.advisories },
          { label: "Ultimo", get: (r) => ago(r.last) },
        ], f.sessions || [], { empty: "Focus no ha emitido avisos (sin actividad visible)" }),
        h2("Codigos de aviso"),
        bars(f.codes, "sin avisos"),
        note(d.note),
      ];
    },
  };

  async function sessionDetail(id) {
    const d = await api("/api/session?" + qs({ id: id }));
    if (d.error) return [el("a", { href: "#/sesiones", text: "< Sesiones" }), el("p", { class: "deny", text: d.error })];
    const s = d.session;
    return [
      el("a", { href: "#/sesiones", text: "< Sesiones" }),
      el("div", { class: "grid" }, [
        card("Sesion", s.session_id.slice(0, 16), s.harness + " - proyecto " + s.project),
        card("Eventos", fmtNum(s.events), s.closed ? "cerrada" : "abierta"),
        card("Primero", ago(s.first_seen), fmtTime(s.first_seen)),
        card("Ultimo", ago(s.last_seen), fmtTime(s.last_seen)),
      ]),
      h2("Linea de tiempo" + (d.truncated ? " (ultimos " + d.timeline.length + ")" : "")),
      table(eventCols(false), d.timeline || [], { nw: true, empty: "sin eventos" }),
    ];
  }

  function approvalButtons(run, t) {
    const act = (grant) => async () => {
      const typed = window.prompt((grant ? "Aprobar" : "Denegar") + " la tarea '" + t.id + "' del run " + run.run_id + ".\n\nMotivo: " + t.approval_reason + "\n\nEscribe el id de la tarea para confirmar:");
      if (typed === null) return;
      try {
        await api("/api/orbit/resolve", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ run: run.run_id, task: t.id, grant: grant, confirm: typed }) });
        await show();
      } catch (e) { window.alert("No realizado: " + e.message); }
    };
    return el("div", { class: "row" }, [el("button", { text: "Aprobar", onclick: act(true) }), el("button", { class: "danger", text: "Denegar", onclick: act(false) })]);
  }

  function parseHash() {
    const h = location.hash.replace(/^#\/?/, "");
    const i = h.indexOf("/");
    const name = (i < 0 ? h : h.slice(0, i)) || "resumen";
    let arg = i < 0 ? "" : h.slice(i + 1);
    try { arg = decodeURIComponent(arg); } catch (_) { /* keep raw */ }
    return { name: renderers[name] ? name : "resumen", arg: arg };
  }

  async function refreshHeader() {
    try {
      const o = await api("/api/summary");
      actions = !!o.actions_enabled;
      const lv = o.health.level;
      dot.className = "dot " + lv;
      healthText.textContent = { ok: "Todo en orden", warn: "Atencion", bad: "Problema" }[lv] + " - v" + o.version + (o.daemon && o.daemon.state === "running" ? " - daemon " + o.daemon.version : " - daemon detenido");
      clear(warningsEl);
      for (const w of o.health.warnings) warningsEl.appendChild(el("div", { class: "banner " + (lv === "bad" ? "bad" : ""), text: w }));
    } catch (e) {
      dot.className = "dot bad";
      healthText.textContent = "UI sin conexion: " + e.message;
    }
  }

  function userIsTyping() {
    const a = document.activeElement;
    return a && view.contains(a) && (a.tagName === "INPUT" || a.tagName === "SELECT");
  }

  async function show() {
    current = parseHash();
    for (const a of document.querySelectorAll("#tabs a")) a.classList.toggle("active", a.dataset.tab === current.name);
    try {
      const nodes = await renderers[current.name](current.arg);
      clear(view);
      for (const n of nodes) if (n) view.appendChild(n);
      statusEl.textContent = "actualizado " + new Date().toLocaleTimeString();
    } catch (e) {
      clear(view);
      view.appendChild(el("p", { class: "deny", text: "No se pudo cargar: " + e.message }));
      statusEl.textContent = "error";
    }
    refreshHeader();
  }

  window.addEventListener("hashchange", show);
  show();
  // Light polling, only while this tab is visible; never touches the daemon beyond a ping.
  const timer = setInterval(() => { if (!document.hidden && !userIsTyping()) show(); }, POLL_MS);
  document.addEventListener("visibilitychange", () => { if (!document.hidden) show(); });
  window.addEventListener("beforeunload", () => clearInterval(timer));
})();
