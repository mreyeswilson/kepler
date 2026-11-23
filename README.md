# Kepler

**Intelligent Execution Control for Coding Agents** — runtime local-first (Rust) que reduce desvíos, trabajo innecesario, afirmaciones sin evidencia y tiempos excesivos de los agentes de código.

> **Estado honesto:** el núcleo Rust, los módulos Forge, Memory, Guardian, Focus, Sentinel, Vision (gestor + worker), Orbit, el servidor MCP mínimo y el adaptador Pi tienen código y tests automatizados que pasan. **Nada se ha verificado contra una sesión real de Pi, un cliente MCP de terceros, un navegador Chromium real ni Linux/macOS.** No hay binarios publicados, ni paquete npm, ni licencia elegida. Detalle por componente más abajo.

## Qué hace hoy

| Módulo | Qué implementa (con tests) | Límites conocidos |
|---|---|---|
| **Focus** | Supervisión determinista como *avisos* no bloqueantes: scope lock, bucles (llamadas/fallos repetidos, ediciones oscilantes), presupuesto adaptativo, guardia de cambio mínimo (manifiestos de dependencias, cambios grandes), puerta de evidencia/finalización. [ADR-0014](docs/decisions/0014-focus-implementation.md) | Umbrales sin calibrar; nunca cancela ni bloquea; el adaptador Pi aún no muestra los avisos; sin benchmark con/sin Kepler |
| **Forge** | Clasificador passthrough/enrich/compile/decompose y spec derivada etiquetada (nunca promueve inferencias a instrucciones). [ADR-0009](docs/decisions/0009-module-pipeline-and-forge.md) | Heurísticas en inglés/español sin evaluar sobre prompts reales; sin LLM |
| **Memory** | FTS5 con alcance, ranking, dedupe/supersession, presupuesto de tokens estimados, controles `kepler memory ...`. [ADR-0010](docs/decisions/0010-memory-storage-and-ranking.md) | Pesos sin afinar; sin embeddings |
| **Guardian** | Preflight de herramientas *fail-closed*, reglas del usuario, secretos en resultados (`redact`), presupuesto de contexto. [ADR-0011](docs/decisions/0011-guardian-and-wire-additions.md) | Coincidencia de cadenas, no es un sandbox; sin tokenizador real; sin compresión de salida |
| **Sentinel** | Auditoría determinista: reglas de rutas en el daemon (`files.changed`) y reglas de líneas añadidas sobre `git diff` (`kepler sentinel audit`); hallazgos `confirmed`/`suspected` en `audit_findings`. [ADR-0015](docs/decisions/0015-sentinel-and-findings.md) | Sin AST ni integración con linters/tests del proyecto; reglas simples |
| **Vision** | Gestor Rust (lazy, timeouts con kill, allowlist localhost, entorno mínimo) + worker TypeScript/Playwright privado en `workers/vision`. [ADR-0016](docs/decisions/0016-vision-worker.md) | **El worker nunca se ejecutó contra un Chromium real** (no se descargó ningún navegador); apagado por defecto hasta configurar `KEPLER_VISION_WORKER` |
| **Orbit** | Planificador DAG de tareas de comandos, concurrencia acotada, un git worktree por tarea, aprobaciones para comandos destructivos, `kepler orbit ...`. [ADR-0017](docs/decisions/0017-orbit-dag-scheduler.md) | No inicia agentes por sí mismo; sin límites de CPU/RAM del SO; el kill de procesos en Unix no se ejecutó fuera de Windows |
| **Genesis** | **No implementado.** Solo diseño y requisitos previos. [ADR-0018](docs/decisions/0018-genesis-deferred-design.md) | No existe UI de escritorio |

Infraestructura: eventos v1 con fixtures, 8 capacidades negociadas ([ADR-0002](docs/decisions/0002-adapter-capabilities.md)), IPC local de usuario único ([ADR-0007](docs/decisions/0007-ipc-wire-protocol.md)), SQLite WAL + FTS5 con redacción de secretos, daemon lazy con apagado por inactividad, servidor MCP stdio ([ADR-0012](docs/decisions/0012-mcp-stdio-minimal-server.md)), adaptador Pi con hooks automáticos (`adapters/pi`, `private: true`, [ADR-0013](docs/decisions/0013-pi-automatic-hooks.md)). Mediciones reales en [PERFORMANCE.md](docs/development/PERFORMANCE.md).

## Instalación (desde el código fuente)

No hay binarios ni paquetes publicados. Requisitos: Rust estable >= 1.89 (`rustup`), Git; Node >= 22.18 solo para `adapters/pi` y `workers/vision`.

```bash
git clone <repo> && cd kepler
cargo build --release            # binario: target/release/kepler (kepler.exe en Windows)
cargo test --workspace           # los tests de Orbit/Sentinel usan `git` real en repos temporales
```

Playwright (opcional, solo para Vision): `cd workers/vision && npm ci --ignore-scripts && npm run build && npx playwright install chromium` (la descarga del navegador la haces tú; Kepler nunca la hace).

## Uso

```bash
kepler doctor                                  # diagnóstico de solo lectura
kepler emit --in-process --json '{...evento v1...}'   # evalúa un evento sin daemon
kepler daemon                                  # normalmente se inicia solo (lazy) con `emit`
kepler inspect                                 # eventos recientes y métricas (solo lectura)
kepler memory list|search|export|add|purge|disable|enable|stats
kepler mcp serve                               # MCP por stdio: kepler_doctor, kepler_inspect, kepler_memory_search

kepler sentinel audit --repo . [--staged | --base main] [--store] [--fail-on high]
kepler sentinel list [--min-severity medium] ; kepler sentinel dismiss <id>

KEPLER_VISION_WORKER=workers/vision/dist/src/main.js kepler vision check http://localhost:3000
kepler vision status [--probe] ; kepler vision clean

kepler orbit plan plan.json                    # valida (ciclos, límites, secretos, Guardian); no ejecuta
kepler orbit run plan.json --repo . --base HEAD
kepler orbit approve|deny <run> <task> ; kepler orbit resume <run>
kepler orbit status <run> ; kepler orbit cancel <run> ; kepler orbit cleanup <run> --yes
```

Ejemplo de `plan.json` (comandos como *argv*, nunca cadenas de shell):

```json
{ "name": "refactor", "max_parallel": 2, "tasks": [
  { "id": "build", "command": ["cargo", "build"] },
  { "id": "test",  "command": ["cargo", "test"], "depends_on": ["build"], "timeout_secs": 900 },
  { "id": "wipe",  "command": ["git", "clean", "-fd"], "depends_on": ["build"] } ] }
```
`wipe` es destructivo: queda en `awaiting_approval` hasta `kepler orbit approve <run> wipe`. Cada tarea trabaja en su propio worktree/rama (`kepler/orbit/<run>/<task>`); Orbit nunca toca tu rama ni fusiona por ti.

### Variables de entorno

| Variable | Efecto |
|---|---|
| `KEPLER_HOME` | Directorio de datos (por defecto el del SO) |
| `KEPLER_IPC_NAME`, `KEPLER_IDLE_TIMEOUT_SECS` | Endpoint IPC y apagado por inactividad del daemon |
| `KEPLER_FORGE`, `KEPLER_FOCUS`, `KEPLER_SENTINEL`, `KEPLER_GUARDIAN_BUDGET` = `off` | Desactiva ese módulo (el preflight de Guardian no se puede desactivar) |
| `KEPLER_FOCUS_SOFT_TOOL_CALLS`, `KEPLER_FOCUS_SOFT_SECONDS` | Presupuestos blandos de Focus |
| `KEPLER_GUARDIAN_RULES` | Archivo de reglas del usuario (solo endurecen) |
| `KEPLER_MEMORY`, `KEPLER_MEMORY_BUDGET_TOKENS`, `KEPLER_MEMORY_RETENTION_DAYS` | Controles de Memory |
| `KEPLER_VISION_WORKER`, `KEPLER_VISION_NODE`, `KEPLER_VISION_ALLOW_HOSTS` | Worker de Vision (apagado sin `WORKER`) y hosts extra permitidos |

## Arquitectura

```text
Harness (Pi) -> adaptador (hooks) --IPC--> kepler daemon (Rust, lazy)
                                             |  PolicyEngine: original -> Forge -> Sentinel -> Memory -> Focus -> Guardian
                                             |  decisión (passthrough/enrich/require_approval/deny/redact) + avisos
                                             +-> SQLite (eventos solo metadatos, memorias, findings, tareas Orbit)
CLI: doctor | emit | inspect | memory | mcp | sentinel | vision | orbit      (sin daemon: acceden a SQLite/git directamente)
Vision: kepler vision --stdio JSON--> worker Node + Playwright (bajo demanda, nunca residente)
Orbit:  DAG -> git worktree por tarea -> procesos hijos supervisados (timeout, kill de árbol, aprobaciones)
```

Crates: `kepler-events` (envelope v1), `-policy` (motor, Guardian, redacción), `-storage` (SQLite, migraciones v1-v4), `-forge`, `-memory`, `-focus`, `-sentinel`, `-git`, `-vision`, `-orbit`, `-mcp`, `-ipc`, `-core` (daemon), `-cli`. Reglas: Rust estable, Tokio para E/S, colas y concurrencia acotadas, nada pesado en reposo, sin dependencias de nube, fallo abierto para optimizaciones y cerrado para seguridad ([AGENTS.md](AGENTS.md)).

## Principios

1. Automático pero proporcional: nada se ejecuta si no aporta valor.
2. Portable, no idéntico: nunca fingir interceptación profunda vía MCP.
3. Local-first: workers y modelos bajo demanda; ninguna llamada de red por defecto.
4. Conservar la intención: el texto original nunca se reescribe; lo derivado va aparte y etiquetado.
5. No persistir secretos, prompts ni salida de herramientas; redacción antes de guardar.
6. Fail-open para mejoras opcionales, fail-closed para operaciones peligrosas.

## Qué falta (sin verificar o sin implementar)

* Verificación con Pi real, cliente MCP de terceros, Chromium real, Linux y macOS (el código Unix del transporte y del kill de procesos de Orbit no se ejecutó fuera de Windows; lo cubrirá la matriz de CI).
* Benchmark con/sin Kepler (KPL-032) y calibración de Focus; la compresión de salida de herramientas; detección/instalación de harnesses; binarios y paquete npm (bloqueados por licencia y verificación real).
* Genesis y cualquier UI de escritorio.

## Documentación

[Visión](docs/VISION.md) · [Arquitectura](docs/architecture/OVERVIEW.md) · [Eventos y capacidades](docs/architecture/EVENTS_AND_CAPABILITIES.md) · [Adaptadores](docs/architecture/HARNESSES.md) · [Focus](docs/architecture/FOCUS.md) · [Módulos](docs/modules/README.md) · [Genesis](docs/modules/GENESIS.md) · [Modelo de amenazas](docs/security/THREAT_MODEL.md) · [Roadmap](docs/roadmap/MVP.md) · [Backlog y estado](docs/roadmap/BACKLOG.md) · [Decisiones (ADR)](docs/decisions/README.md) · [Guía de desarrollo](docs/development/GETTING_STARTED.md) · [Rendimiento](docs/development/PERFORMANCE.md) · [Benchmarks](docs/quality/BENCHMARKS.md) · [Releases](RELEASING.md) · [Cambios](CHANGELOG.md) · [Contribuir](CONTRIBUTING.md)

## Licencia

Pendiente de elegir; los crates declaran `UNLICENSED` y `publish = false`. No asumas derechos de reutilización hasta que exista un archivo `LICENSE`.
