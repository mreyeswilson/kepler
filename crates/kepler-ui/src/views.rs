//! Read-only JSON views for the UI tabs. Everything is derived from metadata Kepler already
//! stores (event kinds, decisions, reason codes, module attribution, counters). No prompt text,
//! tool output, command lines or memory text is ever returned from here. Every list is bounded.

use crate::UiConfig;
use kepler_memory::{MemoryAdmin, MemoryConfig};
use kepler_storage::{inspect, Cell, Reader, LATEST_VERSION};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap};

pub type Query = HashMap<String, String>;

const WINDOW_7D: &str = "strftime('%Y-%m-%dT%H:%M:%fZ','now','-7 days')";
const MAX_OFFSET: u64 = 1_000_000;

fn e<T: ToString>(x: T) -> String {
    x.to_string()
}

fn num(q: &Query, key: &str, default: u64, max: u64) -> u64 {
    q.get(key)
        .and_then(|v| v.parse::<u64>().ok())
        .map_or(default, |n| {
            n.clamp(if key == "limit" { 1 } else { 0 }, max)
        })
}

/// Short, non-reversible-looking handle for a project id (ids are already hashes).
fn alias(project: &str) -> String {
    project.chars().take(11).collect()
}

fn short(id: &str) -> String {
    id.chars().take(8).collect()
}

fn token_ok(s: &str, max: usize) -> bool {
    !s.is_empty()
        && s.len() <= max
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-' | ' '))
}

fn time_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.chars()
            .all(|c| c.is_ascii_digit() || matches!(c, 'T' | 'Z' | ':' | '.' | '-'))
}

fn window_sql(w: &str) -> Option<&'static str> {
    Some(match w {
        "5m" => "strftime('%Y-%m-%dT%H:%M:%fZ','now','-5 minutes')",
        "15m" => "strftime('%Y-%m-%dT%H:%M:%fZ','now','-15 minutes')",
        "1h" => "strftime('%Y-%m-%dT%H:%M:%fZ','now','-1 hours')",
        "24h" => "strftime('%Y-%m-%dT%H:%M:%fZ','now','-24 hours')",
        "7d" => "strftime('%Y-%m-%dT%H:%M:%fZ','now','-7 days')",
        _ => return None,
    })
}

struct Db {
    r: Reader,
    attr: bool,
}

impl Db {
    fn open(cfg: &UiConfig) -> Result<Option<Self>, String> {
        if !cfg.db_path.exists() {
            return Ok(None);
        }
        let r = Reader::open(&cfg.db_path).map_err(e)?;
        let v = r.schema_version().map_err(e)?;
        // The UI never migrates: an older schema just has less attribution.
        Ok(Some(Self { r, attr: v >= 5 }))
    }

    fn rows(&self, sql: &str, p: &[Cell], max: usize) -> Result<Vec<Vec<Cell>>, String> {
        self.r.rows(sql, p, max).map_err(e)
    }

    fn one(&self, sql: &str, p: &[Cell]) -> Result<Vec<Cell>, String> {
        Ok(self.rows(sql, p, 1)?.into_iter().next().unwrap_or_default())
    }

    fn count(&self, sql: &str, p: &[Cell]) -> Result<i64, String> {
        Ok(self.one(sql, p)?.first().map_or(0, Cell::int))
    }

    fn attr_cols(&self) -> &'static str {
        if self.attr {
            "actors, detail, proc_ms"
        } else {
            "NULL, NULL, NULL"
        }
    }
}

fn kv(detail: &str) -> Vec<(&str, &str)> {
    detail
        .split(';')
        .filter_map(|p| p.split_once('='))
        .collect()
}

fn get<'a>(pairs: &'a [(&'a str, &'a str)], key: &str) -> Option<&'a str> {
    pairs.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

fn bump(m: &mut BTreeMap<String, i64>, k: &str, n: i64) {
    *m.entry(k.to_string()).or_insert(0) += n;
}

fn map_json(m: &BTreeMap<String, i64>) -> Value {
    Value::Object(m.iter().map(|(k, v)| (k.clone(), json!(v))).collect())
}

fn pairs_json(rows: &[Vec<Cell>]) -> Vec<Value> {
    rows.iter()
        .map(|r| json!({"name": r[0].text(), "count": r[1].int()}))
        .collect()
}

// ---------------------------------------------------------------- events

fn event_json(r: &[Cell]) -> Value {
    json!({
        "id": r[0].int(), "event_id": r[1].text(), "kind": r[2].text(), "at": r[3].text(),
        "harness": r[4].text(), "project": alias(&r[5].text()), "session_id": r[6].text(),
        "session": short(&r[6].text()), "decision": r[7].text(), "reason": r[8].opt_text(),
        "payload_bytes": r[9].int(), "actors": r[10].opt_text(), "detail": r[11].opt_text(),
        "ms": r[12].opt_int(),
    })
}

const EVENT_COLS: &str =
    "id, event_id, kind, received_at, harness, project_id, session_id, decision, reason, payload_bytes";

fn events(db: &Db, q: &Query) -> Result<Value, String> {
    let mut wh: Vec<String> = Vec::new();
    let mut p: Vec<Cell> = Vec::new();
    let mut eq = |col: &str, key: &str, max: usize| {
        if let Some(v) = q.get(key).filter(|v| token_ok(v, max)) {
            wh.push(format!("{col} = ?"));
            p.push(Cell::Text(v.clone()));
        }
    };
    eq("kind", "kind", 40);
    eq("decision", "decision", 24);
    eq("harness", "harness", 40);
    if let Some(v) = q.get("session").filter(|v| token_ok(v, 128)) {
        wh.push("session_id = ?".into());
        p.push(Cell::Text(v.clone()));
    }
    if db.attr {
        if let Some(m) = q.get("module").filter(|m| {
            m.chars().all(|c| c.is_ascii_alphanumeric()) && !m.is_empty() && m.len() <= 24
        }) {
            wh.push("actors LIKE ?".into());
            p.push(Cell::Text(format!("%{m}%")));
        }
    }
    if let Some(w) = q.get("window").and_then(|w| window_sql(w)) {
        wh.push(format!("received_at >= {w}"));
    }
    if let Some(s) = q.get("since").filter(|s| time_ok(s)) {
        wh.push("received_at >= ?".into());
        p.push(Cell::Text(s.clone()));
    }
    if let Some(s) = q.get("until").filter(|s| time_ok(s)) {
        wh.push("received_at <= ?".into());
        p.push(Cell::Text(s.clone()));
    }
    let wsql = if wh.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", wh.join(" AND "))
    };
    let limit = num(q, "limit", 50, 200);
    let offset = num(q, "offset", 0, MAX_OFFSET);
    let total = db.count(&format!("SELECT COUNT(*) FROM events{wsql}"), &p)?;
    let sql = format!(
        "SELECT {EVENT_COLS}, {} FROM events{wsql} ORDER BY id DESC LIMIT {limit} OFFSET {offset}",
        db.attr_cols()
    );
    let rows = db.rows(&sql, &p, limit as usize)?;
    Ok(json!({
        "events": rows.iter().map(|r| event_json(r)).collect::<Vec<_>>(),
        "total": total, "limit": limit, "offset": offset,
        "has_more": (offset as i64 + rows.len() as i64) < total,
        "attribution": db.attr,
        "note": "metadata only: Kepler never stores prompt text or tool output",
    }))
}

// -------------------------------------------------------------- sessions

fn sessions(db: &Db, q: &Query) -> Result<Value, String> {
    let limit = num(q, "limit", 30, 100);
    let offset = num(q, "offset", 0, MAX_OFFSET);
    let total = db.count("SELECT COUNT(*) FROM sessions", &[])?;
    let rows = db.rows(
        &format!(
            "SELECT session_id, harness, project_id, first_seen, last_seen, event_count, closed
             FROM sessions ORDER BY last_seen DESC, rowid DESC LIMIT {limit} OFFSET {offset}"
        ),
        &[],
        limit as usize,
    )?;
    let ids: Vec<Cell> = rows.iter().map(|r| Cell::Text(r[0].text())).collect();
    let mut decisions: HashMap<String, BTreeMap<String, i64>> = HashMap::new();
    let mut prompts: HashMap<String, i64> = HashMap::new();
    if !ids.is_empty() {
        let marks = vec!["?"; ids.len()].join(",");
        let agg = db.rows(
            &format!(
                "SELECT session_id, decision, COUNT(*), SUM(kind='input.received')
                 FROM events WHERE session_id IN ({marks}) GROUP BY session_id, decision"
            ),
            &ids,
            1000,
        )?;
        for a in agg {
            let s = a[0].text();
            bump(
                decisions.entry(s.clone()).or_default(),
                &a[1].text(),
                a[2].int(),
            );
            *prompts.entry(s).or_insert(0) += a[3].int();
        }
    }
    let list: Vec<Value> = rows
        .iter()
        .map(|r| {
            let id = r[0].text();
            json!({
                "session_id": id, "session": short(&id), "harness": r[1].text(),
                "project": alias(&r[2].text()), "first_seen": r[3].text(),
                "last_seen": r[4].text(), "events": r[5].int(), "closed": r[6].int() == 1,
                "prompts": prompts.get(&id).copied().unwrap_or(0),
                "decisions": decisions.get(&id).map(map_json).unwrap_or(json!({})),
            })
        })
        .collect();
    Ok(json!({"sessions": list, "total": total, "limit": limit, "offset": offset}))
}

fn session_detail(db: &Db, q: &Query) -> Result<Value, String> {
    let Some(id) = q.get("id").filter(|v| token_ok(v, 128)) else {
        return Err("expected ?id=<session id>".into());
    };
    let head = db.one(
        "SELECT session_id, harness, project_id, first_seen, last_seen, event_count, closed
         FROM sessions WHERE session_id = ?",
        &[Cell::Text(id.clone())],
    )?;
    if head.is_empty() {
        return Ok(json!({"error": "unknown session"}));
    }
    let n = num(q, "limit", 300, 500);
    let sql = format!(
        "SELECT {EVENT_COLS}, {} FROM events WHERE session_id = ? ORDER BY id DESC LIMIT {n}",
        db.attr_cols()
    );
    let mut rows = db.rows(&sql, &[Cell::Text(id.clone())], n as usize)?;
    rows.reverse();
    Ok(json!({
        "session": {"session_id": head[0].text(), "harness": head[1].text(),
            "project": alias(&head[2].text()), "first_seen": head[3].text(),
            "last_seen": head[4].text(), "events": head[5].int(), "closed": head[6].int() == 1},
        "timeline": rows.iter().map(|r| event_json(r)).collect::<Vec<_>>(),
        "truncated": head[5].int() > rows.len() as i64,
    }))
}

// --------------------------------------------------------------- summary

pub fn summary(cfg: &UiConfig, daemon: Value) -> Result<Value, String> {
    let st = inspect(&cfg.db_path);
    let mut warnings: Vec<String> = Vec::new();
    let mut level = "ok";
    let mut raise = |lv: &'static str, msg: String| {
        warnings.push(msg);
        if lv == "bad" || level == "ok" {
            level = lv;
        }
    };
    if !st.exists {
        raise(
            "warn",
            "La base de datos aun no existe (el daemon la crea al primer evento).".into(),
        );
    } else if let Some(err) = &st.error {
        raise(
            "bad",
            format!("La base de datos esta corrupta o ilegible: {err}"),
        );
    } else if st.schema_version.unwrap_or(0) > LATEST_VERSION {
        raise(
            "bad",
            "La base tiene un esquema mas nuevo que esta UI.".into(),
        );
    } else if st.schema_version.unwrap_or(0) < LATEST_VERSION {
        raise(
            "warn",
            "Esquema antiguo: el daemon migrara al arrancar; la atribucion por modulo es parcial."
                .into(),
        );
    }
    let ui_version = env!("CARGO_PKG_VERSION");
    match daemon["state"].as_str() {
        Some("running") => {
            if daemon["version"].as_str() != Some(ui_version) {
                raise(
                    "warn",
                    format!(
                        "El daemon es de otra version ({}) que la UI ({ui_version}).",
                        daemon["version"].as_str().unwrap_or("?")
                    ),
                );
            }
        }
        _ => raise(
            "warn",
            "El daemon no esta en ejecucion (los hooks lo arrancan al primer evento).".into(),
        ),
    }
    let mut events = json!({"total": 0, "last_minute": 0, "per_minute_10m": 0.0});
    let mut harnesses = Vec::new();
    if st.exists && st.error.is_none() {
        if let Some(db) = Db::open(cfg)? {
            let total = db.count("SELECT COUNT(*) FROM events", &[])?;
            let lastmin = db.count(
                "SELECT COUNT(*) FROM events WHERE received_at >= strftime('%Y-%m-%dT%H:%M:%fZ','now','-1 minutes')",
                &[],
            )?;
            let ten = db.count(
                "SELECT COUNT(*) FROM events WHERE received_at >= strftime('%Y-%m-%dT%H:%M:%fZ','now','-10 minutes')",
                &[],
            )?;
            events = json!({"total": total, "last_minute": lastmin,
                "per_minute_10m": (ten as f64) / 10.0});
            for r in db.rows(
                "SELECT harness, COUNT(*), COUNT(DISTINCT session_id), MAX(received_at)
                 FROM events GROUP BY harness ORDER BY MAX(received_at) DESC",
                &[],
                20,
            )? {
                harnesses.push(json!({"harness": r[0].text(), "events": r[1].int(),
                    "sessions": r[2].int(), "last_event": r[3].text()}));
            }
        }
    }
    let user = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_default();
    let host = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_default();
    Ok(json!({
        "version": ui_version,
        "actions_enabled": cfg.allow_actions,
        "identity": {"user": user, "host": host, "os": std::env::consts::OS, "arch": std::env::consts::ARCH},
        "daemon": daemon,
        "database": {"path": st.path, "exists": st.exists, "size_bytes": st.size_bytes,
            "schema_version": st.schema_version, "latest_schema": LATEST_VERSION,
            "journal_mode": st.journal_mode, "error": st.error},
        "events": events, "harnesses": harnesses,
        "health": {"level": level, "warnings": warnings},
    }))
}

// --------------------------------------------------------------- modules

struct Meta {
    id: &'static str,
    name: &'static str,
    what: &'static str,
    implemented: &'static str,
    limitation: &'static str,
}

const MODULES: [Meta; 8] = [
    Meta {
        id: "focus",
        name: "Focus",
        what: "Control de ejecucion: mantiene el objetivo original, vigila desvios de alcance, bucles y presupuestos, y recomienda parar al cumplir los criterios.",
        implemented: "si (avisos no bloqueantes)",
        limitation: "Solo emite avisos informativos; nunca bloquea ni cancela. Umbrales sin calibrar. Sin avisos registrados se muestra 'sin actividad'.",
    },
    Meta {
        id: "forge",
        name: "Forge",
        what: "Compilador de prompts: clasifica cada entrada (passthrough, enrich, compile, decompose) y anade contexto derivado etiquetado sin tocar el original.",
        implemented: "si (reglas deterministas + modelo opcional)",
        limitation: "Solo agrega contexto: no reescribe el prompt. El modelo opcional usa tu `claude -p` y solo en casos dudosos; los intents del modelo no se guardan en la base.",
    },
    Meta {
        id: "memory",
        name: "Memory",
        what: "Contexto y conocimiento local: recuperacion FTS5 por proyecto, consolidacion con procedencia, sin guardar chats ni secretos.",
        implemented: "si",
        limitation: "Solo inyecta recuerdos cuando hay coincidencias; las memorias derivadas son notas de baja confianza, nunca instrucciones.",
    },
    Meta {
        id: "guardian",
        name: "Guardian",
        what: "Seguridad y optimizacion de tokens/contexto: preflight de herramientas, escaneo de resultados y presupuesto de contexto.",
        implemented: "parcial",
        limitation: "Hoy es preflight de seguridad + presupuesto de contexto. La compresion/optimizacion de tokens de las salidas de herramientas NO esta implementada.",
    },
    Meta {
        id: "orbit",
        name: "Orbit",
        what: "Orquestacion multiagente: DAG de tareas, concurrencia acotada, worktrees, reintentos, aprobaciones.",
        implemented: "parcial (ejecutor por CLI)",
        limitation: "Hoy es un ejecutor de tareas de comandos por CLI (`kepler orbit`); no se invoca desde el flujo de Claude Code ni lanza agentes por si mismo.",
    },
    Meta {
        id: "sentinel",
        name: "Sentinel",
        what: "Auditoria de codigo: reglas deterministas sobre rutas y diffs; hallazgos separados en confirmados y sospechados.",
        implemented: "parcial (reglas deterministas)",
        limitation: "Solo corre en el flujo cuando llegan eventos files.changed; en el resto es manual por CLI. Sin Tree-sitter, revision con LLM ni integracion con lint/tests.",
    },
    Meta {
        id: "vision",
        name: "Vision",
        what: "Inspeccion de UI: worker bajo demanda con navegador, capturas y chequeos de desbordamiento/accesibilidad.",
        implemented: "parcial (opt-in)",
        limitation: "Apagado hasta `kepler vision setup`; se invoca por CLI/MCP, no desde el flujo de Claude Code. Verificado en Windows 11 con Edge.",
    },
    Meta {
        id: "genesis",
        name: "Genesis",
        what: "Disenador de interfaces con IA: especificaciones, mockups y plan de componentes.",
        implemented: "no (solo este panel)",
        limitation: "Hoy solo existe este panel de visualizacion. El disenador de interfaces NO esta implementado (solo diseno).",
    },
];

struct Ev {
    total: i64,
    last: Option<String>,
    cc: i64,
    last_cc: Option<String>,
    cc7: i64,
}

fn evidence(db: &Db, pred: &str) -> Result<Ev, String> {
    let r = db.one(
        &format!(
            "SELECT COUNT(*), MAX(received_at), COALESCE(SUM(harness='claude-code'),0),
             MAX(CASE WHEN harness='claude-code' THEN received_at END),
             COALESCE(SUM(harness='claude-code' AND received_at >= {WINDOW_7D}),0)
             FROM events WHERE {pred}"
        ),
        &[],
    )?;
    Ok(Ev {
        total: r[0].int(),
        last: r[1].opt_text(),
        cc: r[2].int(),
        last_cc: r[3].opt_text(),
        cc7: r[4].int(),
    })
}

fn counter(label: &str, value: i64) -> Value {
    json!({"label": label, "value": value})
}

fn modules(cfg: &UiConfig) -> Result<Value, String> {
    let db = Db::open(cfg)?;
    let mut out = Vec::new();
    for m in &MODULES {
        let mut o = Map::new();
        o.insert("id".into(), json!(m.id));
        o.insert("name".into(), json!(m.name));
        o.insert("what".into(), json!(m.what));
        o.insert("implemented".into(), json!(m.implemented));
        o.insert("limitation".into(), json!(m.limitation));
        let mut state = "none";
        let mut last: Option<String> = None;
        let mut last_cc: Option<String> = None;
        let mut counters: Vec<Value> = Vec::new();
        let mut evidence_note = "sin base de datos";
        if m.id == "genesis" {
            state = "not_implemented";
            evidence_note = "no hay nada que medir: no esta implementado";
        } else if let Some(db) = &db {
            let attr = db.attr;
            let (pred, note, call_label): (String, &str, &str) = match m.id {
                "forge" if attr => (
                    "kind='input.received' AND detail LIKE '%forge=%'".into(),
                    "entradas clasificadas (cada prompt no trivial)",
                    "entradas clasificadas",
                ),
                "forge" => (
                    "kind='input.received' AND decision='enrich'".into(),
                    "esquema antiguo: solo entradas enriquecidas (sin saber que modulo)",
                    "entradas enriquecidas",
                ),
                "memory" if attr => (
                    "actors LIKE '%memory%' OR kind='memory.candidate'".into(),
                    "recuerdos inyectados o candidatos recibidos",
                    "inyecciones / candidatos",
                ),
                "memory" => (
                    "kind='memory.candidate'".into(),
                    "esquema antiguo: solo candidatos de memoria",
                    "candidatos",
                ),
                "guardian" => (
                    "kind IN ('tool.call.requested','tool.call.completed')".into(),
                    "preflight / escaneo ejecutados en eventos de herramientas",
                    "revisiones",
                ),
                "focus" if attr => (
                    "actors LIKE '%focus%'".into(),
                    "avisos emitidos (Focus observa todo, solo deja rastro cuando avisa)",
                    "avisos",
                ),
                "focus" => (
                    "0".into(),
                    "esquema antiguo: no se registra quien emitio cada aviso",
                    "avisos",
                ),
                "sentinel" if attr => (
                    "kind='files.changed' OR actors LIKE '%sentinel%'".into(),
                    "eventos files.changed procesados / avisos emitidos",
                    "eventos files.changed",
                ),
                "sentinel" => (
                    "kind='files.changed'".into(),
                    "eventos files.changed",
                    "eventos files.changed",
                ),
                _ => ("0".into(), "no se invoca desde eventos", "invocaciones"),
            };
            let ev = evidence(db, &pred)?;
            last = ev.last.clone();
            last_cc = ev.last_cc.clone();
            evidence_note = note;
            counters.push(counter(call_label, ev.total));
            counters.push(counter("de Claude Code (total)", ev.cc));
            counters.push(counter("de Claude Code (7 dias)", ev.cc7));
            match m.id {
                "forge" if attr => {
                    let enriched = evidence(
                        db,
                        "kind='input.received' AND detail LIKE '%forge=%' AND detail NOT LIKE '%forge=passthrough%'",
                    )?;
                    counters.push(counter(
                        "decisiones (enrich/compile/decompose)",
                        enriched.total,
                    ));
                }
                "forge" => {}
                "memory" => {
                    let admin = MemoryAdmin::new(&cfg.db_path);
                    if let Ok(s) = admin.stats(None) {
                        counters.push(counter("memorias vivas", s.live));
                    }
                }
                "guardian" => {
                    let d = db.count(
                        "SELECT COUNT(*) FROM events WHERE decision IN ('deny','require_approval','redact')",
                        &[],
                    )?;
                    counters.push(counter("decisiones (deny/ask/redact)", d));
                }
                "sentinel" | "vision" => {}
                _ => {}
            }
            let (findings, f_last) = if m.id == "sentinel" || m.id == "vision" {
                let r = db.one(
                    "SELECT COUNT(*), MAX(ts) FROM audit_findings WHERE source = ? AND status = 'open'",
                    &[Cell::Text(m.id.into())],
                )?;
                (r[0].int(), r[1].opt_text())
            } else {
                (0, None)
            };
            if m.id == "sentinel" || m.id == "vision" {
                counters.push(counter("hallazgos abiertos", findings));
                if f_last > last {
                    last = f_last;
                }
            }
            if m.id == "orbit" {
                let runs = db.count("SELECT COUNT(*) FROM orbit_runs", &[])?;
                let tasks = db.count("SELECT COUNT(*) FROM tasks", &[])?;
                let wait = db.count(
                    "SELECT COUNT(*) FROM tasks WHERE status='awaiting_approval'",
                    &[],
                )?;
                counters = vec![
                    counter("runs", runs),
                    counter("tareas", tasks),
                    counter("esperando aprobacion", wait),
                ];
                last = db
                    .one("SELECT MAX(created_at) FROM orbit_runs", &[])?
                    .first()
                    .and_then(Cell::opt_text);
                evidence_note = "runs guardados por la CLI (no hay eventos del flujo)";
                state = if runs > 0 { "manual" } else { "none" };
            } else if m.id == "vision" {
                state = if findings > 0 { "manual" } else { "none" };
            } else if ev.cc7 > 0 {
                state = "active";
            } else if ev.total > 0 {
                state = "seen";
            }
            if m.id == "sentinel" && state != "active" && findings > 0 {
                state = "manual";
            }
        }
        o.insert("state".into(), json!(state));
        o.insert("last_activity".into(), json!(last));
        o.insert("last_activity_claude_code".into(), json!(last_cc));
        o.insert("counters".into(), json!(counters));
        o.insert("evidence".into(), json!(evidence_note));
        o.insert(
            "attribution_partial".into(),
            json!(db.as_ref().is_some_and(|d| !d.attr)),
        );
        out.push(Value::Object(o));
    }
    Ok(json!({
        "modules": out,
        "legend": {
            "active": "evidencia de salidas en los ultimos 7 dias con eventos de Claude Code",
            "seen": "hubo actividad, pero no de Claude Code en los ultimos 7 dias",
            "manual": "solo hay rastro de uso manual (CLI/MCP), no del flujo de Claude Code",
            "none": "sin actividad: no hay evidencia en la base",
            "not_implemented": "no implementado",
        },
        "note": "El estado se deriva de eventos reales guardados; no se inventa actividad."
    }))
}

// ----------------------------------------------------------------- forge

fn p95(v: &[u64]) -> Option<u64> {
    if v.is_empty() {
        return None;
    }
    let mut s = v.to_vec();
    s.sort_unstable();
    let idx = ((s.len() as f64) * 0.95).ceil() as usize;
    Some(s[idx.saturating_sub(1).min(s.len() - 1)])
}

fn forge_model(cfg: &UiConfig) -> Value {
    let path = cfg.db_path.parent().map(|d| d.join("forge-model.json"));
    let Some(text) = path
        .filter(|p| std::fs::metadata(p).is_ok_and(|m| m.len() < 512 * 1024))
        .and_then(|p| std::fs::read_to_string(p).ok())
    else {
        return json!({"available": false, "note": "sin llamadas al modelo registradas (estado no encontrado)"});
    };
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        return json!({"available": false, "note": "estado del modelo ilegible"});
    };
    let n = |k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
    let lat: Vec<u64> = v
        .get("lat_ms")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_u64).take(50).collect())
        .unwrap_or_default();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let until = n("disabled_until");
    let calls = n("calls");
    let ok = n("ok");
    json!({
        "available": true, "calls": calls, "ok": ok, "failed": calls.saturating_sub(ok),
        "cache_hits": n("cache_hits"),
        "cache_entries": v.get("cache").and_then(Value::as_array).map_or(0, Vec::len),
        "latency_avg_ms": (!lat.is_empty()).then(|| lat.iter().sum::<u64>() / lat.len() as u64),
        "latency_p95_ms": p95(&lat), "latency_samples": lat.len(),
        "tokens_in": n("tokens_in"), "tokens_out": n("tokens_out"),
        "cost_usd": v.get("cost_usd").and_then(Value::as_f64).filter(|c| c.is_finite()).unwrap_or(0.0),
        "breaker": if now < until { "abierto" } else { "cerrado" },
        "breaker_seconds_left": until.saturating_sub(now),
        "last_error": v.get("last_error").and_then(Value::as_str).unwrap_or("")
            .chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-')).take(48).collect::<String>(),
        "note": "latencia sobre las ultimas muestras guardadas; el estado lo comparten los hooks",
    })
}

fn forge(cfg: &UiConfig) -> Result<Value, String> {
    let model = forge_model(cfg);
    let Some(db) = Db::open(cfg)? else {
        return Ok(json!({"database": false, "model": model}));
    };
    if !db.attr {
        return Ok(
            json!({"database": true, "attribution": false, "model": model,
            "note": "esquema antiguo: la distribucion de modos solo existe desde la v0.0.5"}),
        );
    }
    let rows = db.rows(
        "SELECT detail, COUNT(*) FROM events WHERE kind='input.received' AND detail LIKE '%forge=%'
         GROUP BY detail ORDER BY COUNT(*) DESC",
        &[],
        500,
    )?;
    let (mut modes, mut rules, mut classes, mut confs) = (
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
    );
    let mut total = 0;
    for r in &rows {
        let d = r[0].text();
        let pairs = kv(&d);
        let n = r[1].int();
        total += n;
        bump(&mut modes, get(&pairs, "forge").unwrap_or("?"), n);
        bump(&mut rules, get(&pairs, "rule").unwrap_or("?"), n);
        bump(&mut classes, get(&pairs, "class").unwrap_or("?"), n);
        bump(&mut confs, get(&pairs, "conf").unwrap_or("?"), n);
    }
    let recent = db.rows(
        "SELECT received_at, session_id, payload_bytes, detail FROM events
         WHERE kind='input.received' AND detail LIKE '%forge=%' ORDER BY id DESC LIMIT 25",
        &[],
        25,
    )?;
    let recent: Vec<Value> = recent
        .iter()
        .map(|r| {
            let d = r[3].text();
            let pairs = kv(&d);
            json!({"at": r[0].text(), "session": short(&r[1].text()), "bytes": r[2].int(),
                "mode": get(&pairs, "forge"), "rule": get(&pairs, "rule"),
                "class": get(&pairs, "class"), "confidence": get(&pairs, "conf")})
        })
        .collect();
    Ok(json!({
        "database": true, "attribution": true, "classified_total": total,
        "modes": map_json(&modes), "rules": map_json(&rules), "classes": map_json(&classes),
        "confidence": map_json(&confs), "recent": recent, "model": model,
        "note": "'class' es la clase por reglas (tarea/pregunta/charla); los intents del modelo no se guardan. Solo metadatos: nunca el texto del prompt.",
    }))
}

// -------------------------------------------------------------- guardian

fn guardian(cfg: &UiConfig) -> Result<Value, String> {
    let Some(db) = Db::open(cfg)? else {
        return Ok(json!({"database": false}));
    };
    let decisions = db.rows(
        "SELECT decision, COUNT(*) FROM events WHERE kind IN ('tool.call.requested','tool.call.completed')
         GROUP BY decision ORDER BY COUNT(*) DESC",
        &[],
        10,
    )?;
    let reasons = db.rows(
        "SELECT decision, reason, COUNT(*) FROM events WHERE decision IN ('deny','require_approval','redact')
         GROUP BY decision, reason ORDER BY COUNT(*) DESC",
        &[],
        50,
    )?;
    let mut tools: BTreeMap<String, (i64, i64)> = BTreeMap::new();
    if db.attr {
        for r in db.rows(
            "SELECT detail, decision, COUNT(*) FROM events
             WHERE kind IN ('tool.call.requested','tool.call.completed') AND detail LIKE '%tool=%'
             GROUP BY detail, decision LIMIT 1000",
            &[],
            1000,
        )? {
            let d = r[0].text();
            let tool = get(&kv(&d), "tool").unwrap_or("?").to_string();
            let t = tools.entry(tool).or_insert((0, 0));
            t.0 += r[2].int();
            if r[1].text() != "passthrough" {
                t.1 += r[2].int();
            }
        }
    }
    let mut top: Vec<_> = tools.into_iter().collect();
    top.sort_by_key(|a| std::cmp::Reverse(a.1 .0));
    top.truncate(15);
    let metrics = db.rows(
        "SELECT kind, estimated, COUNT(*), SUM(tokens), SUM(items) FROM context_metrics
         GROUP BY kind, estimated ORDER BY kind, estimated",
        &[],
        20,
    )?;
    let by_decision = db.rows(
        "SELECT decision, COUNT(*) FROM events GROUP BY decision ORDER BY COUNT(*) DESC",
        &[],
        10,
    )?;
    Ok(json!({
        "database": true,
        "tool_decisions": pairs_json(&decisions),
        "all_decisions": pairs_json(&by_decision),
        "reasons": reasons.iter().map(|r| json!({"decision": r[0].text(),
            "reason": r[1].opt_text().unwrap_or_else(|| "(sin motivo)".into()), "count": r[2].int()})).collect::<Vec<_>>(),
        "top_tools": top.iter().map(|(t, (n, f))| json!({"tool": t, "events": n, "flagged": f})).collect::<Vec<_>>(),
        "tools_available": db.attr,
        "token_metrics": metrics.iter().map(|r| json!({"kind": r[0].text(),
            "basis": if r[1].int() == 1 { "estimado" } else { "medido" },
            "samples": r[2].int(), "tokens": r[3].int(), "items": r[4].int()})).collect::<Vec<_>>(),
        "budgets": {"context_tokens": kepler_policy::DEFAULT_CONTEXT_BUDGET_TOKENS,
            "memory_tokens": MemoryConfig::from_env().budget_tokens,
            "note": "presupuestos por defecto (estimados, no medidos); `KEPLER_MEMORY_BUDGET_TOKENS` ajusta Memory"},
        "note": "Las cifras estimadas y medidas nunca se suman. La compresion de salidas no esta implementada.",
    }))
}

// ---------------------------------------------------------------- memory

fn memory(cfg: &UiConfig) -> Result<Value, String> {
    let Some(db) = Db::open(cfg)? else {
        return Ok(json!({"database": false}));
    };
    let admin = MemoryAdmin::new(&cfg.db_path);
    let s = admin.stats(None).map_err(e)?;
    let by_scope = db.rows(
        "SELECT scope, COUNT(*), SUM(superseded_by IS NULL) FROM memories GROUP BY scope",
        &[],
        10,
    )?;
    let by_project = db.rows(
        "SELECT project_id, COUNT(*), SUM(superseded_by IS NULL), MAX(last_used_at), SUM(use_count)
         FROM memories GROUP BY project_id ORDER BY COUNT(*) DESC",
        &[],
        20,
    )?;
    let used = db.rows(
        "SELECT id, scope, category, source, trust, use_count, last_used_at FROM memories
         WHERE last_used_at IS NOT NULL ORDER BY last_used_at DESC LIMIT 20",
        &[],
        20,
    )?;
    let recalls = if db.attr {
        let ev = evidence(&db, "actors LIKE '%memory%'")?;
        json!({"count": ev.total, "last": ev.last})
    } else {
        Value::Null
    };
    let cfgm = MemoryConfig::from_env();
    Ok(json!({
        "database": true,
        "total": s.total, "live": s.live, "superseded": s.superseded, "user_sourced": s.user_sourced,
        "enabled": admin.is_enabled(None).map_err(e)?, "force_disabled_env": cfgm.force_disabled,
        "retention_days": cfgm.retention_days, "max_live_per_project": cfgm.max_live_per_project,
        "by_scope": by_scope.iter().map(|r| json!({"scope": r[0].text(), "total": r[1].int(), "live": r[2].int()})).collect::<Vec<_>>(),
        "by_project": by_project.iter().map(|r| json!({"project": alias(&r[0].text()), "total": r[1].int(),
            "live": r[2].int(), "last_used": r[3].opt_text(), "uses": r[4].int()})).collect::<Vec<_>>(),
        "recent_uses": used.iter().map(|r| json!({"id": r[0].int(), "scope": r[1].text(), "category": r[2].opt_text(),
            "source": r[3].text(), "trust": r[4].int(), "use_count": r[5].int(), "last_used": r[6].opt_text()})).collect::<Vec<_>>(),
        "recalls_in_events": recalls,
        "note": "Metadatos solamente: el texto de las memorias solo se ve en la pestana 'Memorias' (redactado).",
    }))
}

// -------------------------------------------------------------- sentinel

fn sentinel(cfg: &UiConfig) -> Result<Value, String> {
    let Some(db) = Db::open(cfg)? else {
        return Ok(json!({"database": false}));
    };
    let sev = db.rows(
        "SELECT severity, COUNT(*) FROM audit_findings WHERE source='sentinel' AND status='open' GROUP BY severity",
        &[],
        10,
    )?;
    let rules = db.rows(
        "SELECT rule_id, COUNT(*) FROM audit_findings WHERE source='sentinel' AND status='open'
         GROUP BY rule_id ORDER BY COUNT(*) DESC",
        &[],
        20,
    )?;
    let files = db.rows(
        "SELECT path, COUNT(*) FROM audit_findings WHERE source='sentinel' AND status='open'
         GROUP BY path ORDER BY COUNT(*) DESC",
        &[],
        20,
    )?;
    let status = db.rows(
        "SELECT status, COUNT(*) FROM audit_findings WHERE source='sentinel' GROUP BY status",
        &[],
        4,
    )?;
    let latest = db.rows(
        "SELECT ts, rule_id, severity, confidence, path, line, status FROM audit_findings
         WHERE source='sentinel' ORDER BY id DESC LIMIT 30",
        &[],
        30,
    )?;
    let ev = evidence(&db, "kind='files.changed'")?;
    Ok(json!({
        "database": true,
        "by_severity": pairs_json(&sev), "by_rule": pairs_json(&rules),
        "by_file": pairs_json(&files), "by_status": pairs_json(&status),
        "latest": latest.iter().map(|r| json!({"at": r[0].text(), "rule": r[1].text(),
            "severity": r[2].text(), "confidence": r[3].text(), "path": r[4].text(),
            "line": r[5].opt_int(), "status": r[6].text()})).collect::<Vec<_>>(),
        "files_changed_events": {"count": ev.total, "last": ev.last, "claude_code_7d": ev.cc7},
        "note": "Sin mensajes ni evidencia: solo regla, severidad y ubicacion. Descartar hallazgos se hace por CLI.",
    }))
}

// ---------------------------------------------------------- vision/focus

fn exists_any(paths: &[&str]) -> bool {
    paths.iter().any(|p| std::path::Path::new(p).is_file())
}

fn vision_focus(cfg: &UiConfig) -> Result<Value, String> {
    let dir = cfg.db_path.parent().map(|d| d.join("vision-worker"));
    let installed = dir.as_ref().is_some_and(|d| {
        d.join("src").join("main.ts").is_file()
            && d.join("node_modules").join("playwright").is_dir()
    });
    let env_worker = std::env::var_os("KEPLER_VISION_WORKER").is_some_and(|v| !v.is_empty());
    let edge = exists_any(&[
        "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
        "C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe",
        "/usr/bin/microsoft-edge",
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
    ]);
    let chrome = exists_any(&[
        "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
        "C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe",
        "/usr/bin/google-chrome",
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    ]);
    let mut vision = json!({
        "worker_installed": installed || env_worker, "worker_via_env": env_worker,
        "browser": {"edge_found": edge, "chrome_found": chrome,
            "note": "solo se comprueba si existen los ejecutables; la UI no lanza navegadores"},
        "findings": [], "findings_open": 0,
    });
    let mut focus = json!({"events_with_advisories": 0, "sessions": [], "codes": {}});
    if let Some(db) = Db::open(cfg)? {
        let f = db.rows(
            "SELECT ts, rule_id, severity, path, status FROM audit_findings WHERE source='vision'
             ORDER BY id DESC LIMIT 20",
            &[],
            20,
        )?;
        vision["findings"] = json!(f
            .iter()
            .map(|r| json!({"at": r[0].text(), "rule": r[1].text(),
            "severity": r[2].text(), "target": r[3].text(), "status": r[4].text()}))
            .collect::<Vec<_>>());
        vision["findings_open"] = json!(db.count(
            "SELECT COUNT(*) FROM audit_findings WHERE source='vision' AND status='open'",
            &[]
        )?);
        if db.attr {
            let sess = db.rows(
                "SELECT session_id, COUNT(*), MAX(received_at) FROM events WHERE actors LIKE '%focus%'
                 GROUP BY session_id ORDER BY MAX(received_at) DESC LIMIT 20",
                &[],
                20,
            )?;
            let mut codes: BTreeMap<String, i64> = BTreeMap::new();
            for r in db.rows(
                "SELECT detail, COUNT(*) FROM events WHERE actors LIKE '%focus%' AND actors NOT LIKE '%sentinel%'
                 GROUP BY detail LIMIT 500",
                &[],
                500,
            )? {
                let d = r[0].text();
                for (k, v) in kv(&d) {
                    if k == "adv" {
                        bump(&mut codes, v, r[1].int());
                    }
                }
            }
            focus = json!({
                "events_with_advisories": sess.iter().map(|r| r[1].int()).sum::<i64>(),
                "sessions": sess.iter().map(|r| json!({"session_id": r[0].text(), "session": short(&r[0].text()),
                    "advisories": r[1].int(), "last": r[2].text()})).collect::<Vec<_>>(),
                "codes": map_json(&codes),
                "attribution": true,
            });
        } else {
            focus["attribution"] = json!(false);
        }
    }
    Ok(json!({"vision": vision, "focus": focus,
        "note": "Focus solo deja rastro cuando emite un aviso; sin avisos = sin actividad visible."}))
}

// ----------------------------------------------------------------- route

pub fn route(
    cfg: &UiConfig,
    path: &str,
    q: &Query,
    daemon: Option<Value>,
) -> Result<Option<Value>, String> {
    let need_db = |f: &dyn Fn(&Db) -> Result<Value, String>| -> Result<Value, String> {
        match Db::open(cfg)? {
            Some(db) => f(&db),
            None => Ok(json!({"database": false, "note": "sin base de datos todavia"})),
        }
    };
    Ok(Some(match path {
        "/api/summary" => summary(cfg, daemon.unwrap_or(json!({"state": "unknown"})))?,
        "/api/modules" => modules(cfg)?,
        "/api/sessions" => need_db(&|db| sessions(db, q))?,
        "/api/session" => need_db(&|db| session_detail(db, q))?,
        "/api/events" => need_db(&|db| events(db, q))?,
        "/api/forge" => forge(cfg)?,
        "/api/guardian" => guardian(cfg)?,
        "/api/memory" => memory(cfg)?,
        "/api/sentinel" => sentinel(cfg)?,
        "/api/vision-focus" => vision_focus(cfg)?,
        _ => return Ok(None),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsing_helpers_are_strict() {
        assert!(token_ok("session-1.a", 20));
        assert!(!token_ok("a'b", 20));
        assert!(!token_ok("", 20));
        assert!(time_ok("2026-01-02T03:04:05Z"));
        assert!(!time_ok("2026'; DROP"));
        assert_eq!(p95(&[1, 2, 3, 4, 100]), Some(100));
        assert_eq!(p95(&[]), None);
        assert_eq!(alias("cc-0123456789abcdef"), "cc-01234567");
    }

    #[test]
    fn all_eight_modules_are_described_honestly() {
        let names: Vec<_> = MODULES.iter().map(|m| m.name).collect();
        assert_eq!(
            names,
            ["Focus", "Forge", "Memory", "Guardian", "Orbit", "Sentinel", "Vision", "Genesis"]
        );
        assert!(MODULES[3].limitation.contains("NO esta implementada"));
        assert!(MODULES[7].limitation.contains("NO esta implementado"));
    }
}
