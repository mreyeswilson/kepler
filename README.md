# Kepler

**Intelligent Execution Control for Coding Agents** — runtime local-first (Rust) que reduce desvíos, trabajo innecesario, afirmaciones sin evidencia y tiempos excesivos de los agentes de código.

> **Estado honesto:** el núcleo Rust, los módulos Forge, Memory, Guardian, Focus, Sentinel, Vision (gestor + worker), Orbit, el servidor MCP mínimo y el adaptador Pi tienen código y tests automatizados que pasan. **Nada se ha verificado contra una sesión real de Pi, un cliente MCP de terceros, un navegador Chromium real ni Linux/macOS.** No hay release publicado todavía (los instaladores y el pipeline por tags no se han ejecutado contra uno real) ni paquete npm. Licencia: MIT. Detalle por componente más abajo.

## Qué hace hoy

| Módulo | Qué implementa (con tests) | Límites conocidos |
|---|---|---|
| **Focus** | Supervisión determinista como *avisos* no bloqueantes: scope lock, bucles (llamadas/fallos repetidos, ediciones oscilantes), presupuesto adaptativo, guardia de cambio mínimo (manifiestos de dependencias, cambios grandes), puerta de evidencia/finalización. [ADR-0014](docs/decisions/0014-focus-implementation.md) | Calibrado solo con 12 trazas sintéticas ([ADR-0023](docs/decisions/0023-focus-calibration-and-advisory-display.md): 9/9 avisos esperados, 0 falsos positivos, ~84 us/evento); nunca cancela ni bloquea; el adaptador Pi muestra los avisos con `ctx.ui.notify` (probado con UI simulada, no en un TUI real); sin A/B con/sin Kepler con un modelo real |
| **Forge** | Clasificador passthrough/enrich/compile/decompose y spec derivada etiquetada (nunca promueve inferencias a instrucciones). [ADR-0009](docs/decisions/0009-module-pipeline-and-forge.md) | Corpus propio de 79 prompts es/en (`fixtures/forge/prompts.tsv`) con 100 % de acierto tras ajustar reglas: riesgo de sobreajuste, sin prompts de usuarios reales. Reglas primero; en el adaptador de Claude Code, un modelo pequeño opcional resuelve los casos dudosos ([ADR-0026](docs/decisions/0026-forge-model-assisted-intent.md), ver más abajo) |
| **Memory** | FTS5 con alcance, ranking, dedupe/supersession, presupuesto de tokens estimados, controles `kepler memory ...`. [ADR-0010](docs/decisions/0010-memory-storage-and-ranking.md) | Captura automática de lo *observado* (comandos de build/test que terminaron bien; archivos cambiados por rama) y selección real del scope de rama ([ADR-0021](docs/decisions/0021-memory-auto-capture-and-branch.md)); pesos de ranking sin afinar; sin embeddings |
| **Guardian** | Preflight de herramientas *fail-closed*, reglas del usuario, secretos en resultados (`redact`), presupuesto de contexto. [ADR-0011](docs/decisions/0011-guardian-and-wire-additions.md) | Análisis real de comandos shell/PowerShell/cmd (comillas, variables, `bash -c`, `-EncodedCommand`, pipes a intérpretes, symlinks/junctions) con corpus de ~190 casos ([ADR-0020](docs/decisions/0020-guardian-shell-analysis.md)); sigue sin ser un sandbox: no ve scripts ejecutados desde archivos, alias, ni lo que el harness no reporta; ~0,6 ms/comando |
| **Sentinel** | Auditoría determinista: reglas de rutas en el daemon (`files.changed`) y reglas de líneas añadidas sobre `git diff` (`kepler sentinel audit`); hallazgos `confirmed`/`suspected` en `audit_findings`. [ADR-0015](docs/decisions/0015-sentinel-and-findings.md) | `kepler sentinel check` ejecuta lint/test del proyecto (acotado, tras el Guardian) y registra hallazgos ([ADR-0022](docs/decisions/0022-sentinel-project-checks.md)); parsers heurísticos de texto, sin AST; no corre solo desde el daemon |
| **Vision** | Gestor Rust (lazy, timeouts con kill, allowlist localhost, entorno mínimo) + worker TypeScript/Playwright privado en `workers/vision`. [ADR-0016](docs/decisions/0016-vision-worker.md) | **Probado en Windows 11 con Microsoft Edge real** (`scripts/acceptance.ps1`: error de consola, recurso 404, enlace roto, captura PNG, allowlist, timeout con kill del árbol). Necesita Node >= 22.18 y `kepler vision setup`; Linux/macOS **no probados**; sin auditoría de contraste ni árbol de accesibilidad |
| **Orbit** | Planificador DAG de tareas de comandos, concurrencia acotada, un git worktree por tarea, aprobaciones para comandos destructivos, `kepler orbit ...`. [ADR-0017](docs/decisions/0017-orbit-dag-scheduler.md) | **Probado en Windows** con repos git reales (dependencias, commit en la rama de la tarea, aprobación + resume, cancelación con kill del árbol, cleanup) y con `claude -p` real mediante el preset `agent`. Sin límites de CPU/RAM del SO; el kill de procesos en Unix **no se ha ejecutado** |
| **Genesis (UI local)** | `kepler ui`: panel en `127.0.0.1` con token de sesión, comprobación de Host/Origin, API JSON de solo lectura (resumen, eventos, memoria, hallazgos, Orbit) y aprobar/denegar Orbit solo con `--allow-actions` y confirmación escrita. [ADR-0024](docs/decisions/0024-genesis-local-ui.md) | Es un panel, no el flujo de diseño de UI de [ADR-0018](docs/decisions/0018-genesis-deferred-design.md) (sin generación de diseños/código); solo verificado en Windows |

Infraestructura: eventos v1 con fixtures, 8 capacidades negociadas ([ADR-0002](docs/decisions/0002-adapter-capabilities.md)), IPC local de usuario único ([ADR-0007](docs/decisions/0007-ipc-wire-protocol.md)), SQLite WAL + FTS5 con redacción de secretos, daemon lazy con apagado por inactividad, servidor MCP stdio ([ADR-0012](docs/decisions/0012-mcp-stdio-minimal-server.md)), adaptador Pi con hooks automáticos (`adapters/pi`, `private: true`, [ADR-0013](docs/decisions/0013-pi-automatic-hooks.md)). Mediciones reales en [PERFORMANCE.md](docs/development/PERFORMANCE.md).

## Instalación (Install)

> Los instaladores descargan los binarios del último GitHub Release publicado y verifican su SHA-256. Los one-liners funcionan sin token mientras el repo sea público.

### Instalador de una línea (Linux / macOS, x86_64 y aarch64)

```sh
curl -fsSL https://raw.githubusercontent.com/mreyeswilson/kepler/main/install.sh | sh
```

### Windows (PowerShell 5.1+, sin administrador, x86_64)

```powershell
irm https://raw.githubusercontent.com/mreyeswilson/kepler/main/install.ps1 | iex
```

**Revisa el script antes de ejecutarlo** (`curl -fsSL <url> | less`, o descárgalo y léelo): ejecutar código remoto con `curl | sh` / `irm | iex` exige confiar en ese script. Ambos: solo HTTPS (TLS 1.2+), sin telemetría, sin `sudo`/admin, **verifican el SHA-256** del archivo contra `SHA256SUMS` del release y abortan si no coincide, y lo único que ejecutan tras instalar es `kepler version`.

| Variable | Efecto |
|---|---|
| `KEPLER_VERSION` | versión a instalar (`0.1.0` o `v0.1.0`); por defecto, el último release |
| `KEPLER_INSTALL_DIR` | destino. Por defecto `$HOME/.local/bin` (Unix) o `%LOCALAPPDATA%\Programs\kepler\bin` (Windows) |
| `KEPLER_ADD_TO_PATH=1` | solo Windows: **opt-in** para añadir el directorio al PATH de usuario (por defecto no se toca el PATH; se imprimen instrucciones). `KEPLER_NO_MODIFY_PATH=1` lo impide siempre |
| `--uninstall` (sh) / `KEPLER_UNINSTALL=1` (PowerShell) | desinstala: `curl -fsSL <url> | sh -s -- --uninstall`; en PowerShell `$env:KEPLER_UNINSTALL=1` antes del `irm ... | iex` |\| sh -s -- --uninstall`; en PowerShell `$env:KEPLER_UNINSTALL=1` antes del `irm`) |
| `KEPLER_REPO`, `KEPLER_BASE_URL` | repositorio alternativo / base de descarga alternativa (https o `file://`, para espejos y tests) |

`install.sh --help` y `install.ps1 -Help` listan las opciones.

### Descarga manual y verificación

Cada release publica `kepler-<versión>-<target>.tar.gz` (`.zip` en Windows) y `SHA256SUMS`. Targets: `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`, `aarch64-apple-darwin`, `x86_64-pc-windows-msvc`. Cada archivo contiene `kepler[.exe]`, `LICENSE` y `README.md`.

```sh
# 1. checksum
sha256sum --check --ignore-missing SHA256SUMS          # macOS: shasum -a 256 -c --ignore-missing SHA256SUMS
# 2. atestación de procedencia (GitHub CLI) — solo si el release se construyó desde un repo no privado
gh attestation verify kepler-X.Y.Z-<target>.tar.gz --repo mreyeswilson/kepler
```

Los binarios no están firmados/notarizados (Gatekeeper / SmartScreen pueden avisar); la integridad la dan `SHA256SUMS` y, en releases construidos desde un repo público, la atestación (GitHub no ofrece atestaciones en repos privados de usuario, así que los releases hechos mientras el repo es privado no la tienen y `gh attestation verify` no aplica). Proceso de release: [RELEASING.md](RELEASING.md).

### Nota: probar con el repo privado

Solo si el repo vuelve a ser privado: los one-liners `irm`/`curl` desde `raw.githubusercontent.com` **no funcionan** (404 sin autenticación). Usa el instalador desde tu checkout local; si hay token, descarga los assets por la API de GitHub (`api.github.com`, solo HTTPS) y **sigue verificando el SHA-256**. El token se busca en `KEPLER_GITHUB_TOKEN`, `GH_TOKEN`, `GITHUB_TOKEN` y, como último recurso, `gh auth token`; solo se envía a `api.github.com`, nunca se imprime ni se guarda.

```powershell
# Windows (PowerShell), desde el checkout local
$env:KEPLER_GITHUB_TOKEN = (gh auth token)
$env:KEPLER_INSTALL_DIR = "$env:TEMP\kepler-prueba"   # opcional; no toca el PATH
.\install.ps1
```

```sh
# Linux / macOS
KEPLER_GITHUB_TOKEN="$(gh auth token)" sh install.sh
```

La descarga pública anónima se intenta siempre primero; el token solo se usa si defines `KEPLER_GITHUB_TOKEN` o como reintento cuando la descarga anónima es rechazada (401/403/404), en cuyo caso también se buscan `GH_TOKEN`, `GITHUB_TOKEN` y `gh auth token`.

### Desde el código fuente

Requisitos: Rust estable >= 1.89 (`rustup`), Git; Node >= 22.18 solo para `adapters/pi` y `workers/vision`.


```bash
git clone <repo> && cd kepler
cargo build --release            # binario: target/release/kepler (kepler.exe en Windows)
cargo test --workspace           # los tests de Orbit/Sentinel usan `git` real en repos temporales
```

Vision (opcional): ver "Preparar Vision". Desarrollo del worker: `cd workers/vision && npm ci --ignore-scripts && npm run build`. Kepler nunca descarga un navegador; si no tienes Edge/Chrome, `npx playwright install chromium` lo haces tú.

## Uso

```bash
kepler doctor                                  # diagnóstico de solo lectura
kepler emit --in-process --json '{...evento v1...}'   # evalúa un evento sin daemon
kepler daemon                                  # normalmente se inicia solo (lazy) con `emit`
kepler inspect                                 # eventos recientes y métricas (solo lectura)
kepler memory list|search|export|add|purge|disable|enable|stats
kepler mcp serve                               # MCP por stdio: kepler_doctor, kepler_inspect, kepler_memory_search

kepler sentinel audit --repo . [--staged | --base main] [--store] [--fail-on high]
kepler sentinel check --repo . --detect [--cmd "lint=cargo clippy"] [--timeout-secs 300] [--store]
kepler sentinel list [--min-severity medium] ; kepler sentinel dismiss <id>
kepler ui [--port N] [--allow-actions] [--open]  # panel local con token en la URL

kepler vision setup                            # una vez: instala el worker en el directorio de datos (npm install de playwright; sin navegador)
kepler vision status --probe                   # lanza el navegador una vez y dice cuál encontró (Edge, Chrome, Chromium)
kepler vision check http://localhost:3000 [--viewport 375x812] [--screenshot] [--store]
kepler vision clean

kepler orbit plan plan.json                    # valida (ciclos, límites, secretos, Guardian); no ejecuta
kepler orbit run plan.json --repo . --base HEAD
kepler orbit approve|deny <run> <task> ; kepler orbit resume <run>
kepler orbit status <run> ; kepler orbit logs <run> <task> ; kepler orbit cancel <run> ; kepler orbit cleanup <run> --yes
```

### Preparar Vision (Windows)

1. Instala Node.js >= 22.18 (el worker son fuentes TypeScript que Node ejecuta directamente; no hay paso de compilación).
2. `kepler vision setup`: escribe el worker en `%LOCALAPPDATA%\kepler\vision-worker` y ejecuta `npm install --omit=dev --ignore-scripts` solo para `playwright` (versión fijada). Es explícito: no hay `postinstall` oculto y no se descarga ningún navegador.
3. Navegador: se usa uno ya instalado, en este orden: `KEPLER_VISION_BROWSER` (`msedge`, `chrome` o ruta a un ejecutable), Microsoft Edge (viene con Windows 11), Google Chrome, Chromium de Playwright. Si no hay ninguno, `kepler vision status --probe` lo dice (`browser: NONE`, exit 2) y propone instalar Edge/Chrome o ejecutar `npx playwright install chromium`.
4. `kepler vision status --probe` para comprobarlo. `KEPLER_VISION_WORKER=<ruta>` sustituye al worker instalado (desarrollo: `workers/vision`, `npm ci && npm run build`, `dist/src/main.js`).

### Suite de aceptación funcional

`powershell -File scripts/acceptance.ps1 [-Kepler ruta\kepler.exe] [-RequireVision] [-WithClaude]` ejercita el binario release en un `KEPLER_HOME` temporal (version, doctor, emit passthrough/enrich/deny, memory, sentinel audit/check, inspect, MCP por stdio, hook `kepler hook claude-code` en frío (rm -rf => ask, Read .env => deny, `ls` en silencio, JSON basura => exit 0), `kepler ui` con y sin token, Vision contra un servidor local y un navegador real, Orbit con repos git temporales incl. cancelación y tarea `agent`) e imprime una tabla PASS/FAIL/SKIP; sale con 1 si algo falla. Un SKIP (p. ej. sin Node o sin navegador) nunca cuenta como PASS; `-RequireVision` lo convierte en fallo (así lo usa el job `acceptance.yml` en `windows-2022`). `-WithClaude` añade una tarea real `claude -p` no destructiva.

### Tareas de agente en Orbit

Una tarea puede ser `{"id":"fix","agent":"<prompt>","agent_args":["--permission-mode","acceptEdits"],"timeout_secs":900}` en lugar de `command`: ejecuta `claude -p "<prompt>" <agent_args...>` como argv (nunca por un shell) en el worktree de la tarea, con timeout y kill del árbol; lo que el agente deje en el worktree se commitea en `kepler/orbit/<run>/<task>`. `KEPLER_AGENT_BIN` cambia el ejecutable. Salida en `kepler orbit logs`, con secretos redactados al terminar la tarea. Los permisos del agente son los que le des con `agent_args`; Orbit no los amplía.

Ejemplo de `plan.json` (comandos como *argv*, nunca cadenas de shell):

```json
{ "name": "refactor", "max_parallel": 2, "tasks": [
  { "id": "build", "command": ["cargo", "build"] },
  { "id": "test",  "command": ["cargo", "test"], "depends_on": ["build"], "timeout_secs": 900 },
  { "id": "wipe",  "command": ["git", "clean", "-fd"], "depends_on": ["build"] } ] }
```
`wipe` es destructivo: queda en `awaiting_approval` hasta `kepler orbit approve <run> wipe`. Cada tarea trabaja en su propio worktree/rama (`kepler/orbit/<run>/<task>`); Orbit nunca toca tu rama ni fusiona por ti.

### Conectar con Claude Code

Dos piezas independientes: el MCP solo da herramientas; los hooks son lo que observa y protege. Verificado solo con Claude Code 2.1.330 en Windows (ejecución real de `claude -p` en un directorio temporal); no en la UI interactiva; Linux/macOS sin probar ([ADR-0025](docs/decisions/0025-claude-code-hooks-adapter.md)).

**1. MCP** (PowerShell; el registro con `claude mcp add` no se ejecutó en esta verificación; herramientas `kepler_doctor`, `kepler_inspect`, `kepler_memory_search`):

```powershell
claude mcp add kepler -- "C:\ruta\a\kepler.exe" mcp serve
```

**2. Hooks** (sin Node). Genera el bloque con la ruta absoluta de tu `kepler`:

```powershell
kepler hook claude-code --print-settings | Set-Clipboard    # PowerShell
```

Pega la clave `"hooks"` en `.claude\settings.json` de un proyecto (recomendado para probar) o en `%USERPROFILE%\.claude\settings.json`. En Linux/macOS el comando equivale pero no está probado. Kepler **no** edita tus settings. Si tu Claude Code no admite `args`, usa `--shell-form`.

| Hook | Efecto |
|---|---|
| `UserPromptSubmit` | Envía tu prompt tal cual; si Forge/Memory aportan algo, lo añade como contexto etiquetado "derivado, NO instrucciones". Nunca cambia ni bloquea el prompt |
| `PreToolUse` | Guardian: `deny` ante rutas sensibles (`.env`, claves) o reglas `deny-*`; `ask` ante comandos destructivos (`rm -rf /`, `git push --force`). Nunca aprueba por ti |
| `PostToolUse` | Detecta secretos en la salida y te avisa (`systemMessage`); no modifica el resultado ni lo guarda |
| `Stop`, `SessionStart`, `SessionEnd` | Solo observan |

Si Kepler falla (daemon caído, JSON raro, timeout `KEPLER_HOOK_TIMEOUT_MS`, 2000 ms por defecto) el hook sale con código 0 sin salida (Claude Code sigue sin Kepler), salvo `PreToolUse`: ahí Guardian se evalúa localmente (en proceso) para que un daemon frío no deje sin protección. Para bloquear en vez de preguntar, añade `deny-command rm -rf /` a `guardian-rules.txt` (`KEPLER_GUARDIAN_RULES`).

#### Forge con modelo (opcional, activado por defecto en este adaptador)

Forge decide primero con reglas deterministas (también entiende formas conversacionales: "vamos a...", "necesito...", "hay que..."). Solo cuando las reglas **dudan** (texto de al menos dos palabras que no es ni tarea reconocible, ni pregunta clara, ni comando, ni nombra un archivo) el hook `UserPromptSubmit` le pregunta a Haiku mediante **tu propio** `claude -p` (tu inicio de sesión, sin clave nueva) qué intención tiene el prompt. Detalle y decisión: [ADR-0026](docs/decisions/0026-forge-model-assisted-intent.md).

- **Qué se envía a Anthropic, y cuándo:** únicamente el texto de ese prompt dudoso, con secretos redactados y cortado a 2000 caracteres, como una petición de clasificación aparte (se factura a tu uso). Nunca salidas de herramientas, memoria, archivos ni la ruta del proyecto. Los prompts que las reglas deciden (tareas con archivo, preguntas claras, comandos, saludos, formas conversacionales con verbo conocido) no hacen ninguna llamada.
- **Qué hace la respuesta:** se valida contra un esquema cerrado y, si dice "es una tarea", se añade una nota etiquetada `model-inferred` ("parece una tarea de tipo X; no se indicó: objetivo...") dentro del bloque "derivado, NO instrucciones". Tu prompt no se toca y la nota nunca se trata como instrucción tuya.
- **Latencia:** ver CHANGELOG ([Unreleased]) para las cifras medidas. Esos prompts esperan la llamada (plazo `KEPLER_FORGE_MODEL_TIMEOUT_MS`); si vence, se mata el proceso y sigues solo con reglas. Caché por hash del prompt normalizado (no guarda el texto) y cortacircuitos: 2 fallos seguidos apagan el modelo 10 minutos.
- **Desactivar:** `KEPLER_FORGE_MODEL=off` (solo el modelo) o `KEPLER_FORGE=off` (todo Forge). Si `claude` no existe o no hay sesión, el hook sigue solo con reglas, sin error visible. `kepler doctor` muestra estado, binario, cortacircuitos, latencia media, tokens y coste que informa el CLI.
- **Sin recursión:** el `claude -p` hijo corre sin herramientas ni persistencia, con los hooks desactivados y con `KEPLER_FORGE_CHILD=1`, que hace que `kepler hook claude-code` salga de inmediato.

**Desactivar:** borra el bloque `hooks` de Kepler de tu `settings.json` (o `"disableAllHooks": true`) y, para el MCP, `claude mcp remove kepler`. Comprueba la actividad con `kepler inspect`.

### Variables de entorno

| Variable | Efecto |
|---|---|
| `KEPLER_HOME` | Directorio de datos (por defecto el del SO) |
| `KEPLER_HOOK_TIMEOUT_MS` | Plazo total de cada hook de Claude Code (1-10000, 2000 por defecto) |
| `KEPLER_FORGE_MODEL` | `off` desactiva la consulta al modelo de Forge (sí: se activa por defecto); otro valor = alias o id de modelo (`haiku` por defecto) |
| `KEPLER_FORGE_MODEL_TIMEOUT_MS`, `KEPLER_FORGE_BREAKER_MINUTES`, `KEPLER_CLAUDE_BIN` | Plazo de la consulta (10000 por defecto), minutos con el modelo apagado tras 2 fallos seguidos (10) y binario `claude` a usar |
| `KEPLER_IPC_NAME`, `KEPLER_IDLE_TIMEOUT_SECS` | Endpoint IPC y apagado por inactividad del daemon |
| `KEPLER_FORGE`, `KEPLER_FOCUS`, `KEPLER_SENTINEL`, `KEPLER_GUARDIAN_BUDGET` = `off` | Desactiva ese módulo (el preflight de Guardian no se puede desactivar) |
| `KEPLER_FOCUS_SOFT_TOOL_CALLS`, `KEPLER_FOCUS_SOFT_SECONDS` | Presupuestos blandos de Focus |
| `KEPLER_GUARDIAN_RULES` | Archivo de reglas del usuario (solo endurecen) |
| `KEPLER_MEMORY`, `KEPLER_MEMORY_BUDGET_TOKENS`, `KEPLER_MEMORY_RETENTION_DAYS` | Controles de Memory |
| `KEPLER_VISION_WORKER`, `KEPLER_VISION_NODE`, `KEPLER_VISION_ALLOW_HOSTS` | Worker de Vision (por defecto el instalado por `kepler vision setup`), Node a usar y hosts extra permitidos |
| `KEPLER_VISION_BROWSER`, `KEPLER_NPM` | Navegador de Vision (`msedge`, `chrome`, `chromium` o ruta a un ejecutable) y npm usado por `vision setup` |
| `KEPLER_AGENT_BIN` | Ejecutable de las tareas `agent` de Orbit (por defecto `claude`) |

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

* **Estado integrado (rama `integration/all`, solo Windows 11, Node 24):** `cargo clippy -D warnings`, `cargo test --workspace`, tests y e2e de `adapters/pi` (24/24 con Pi 1.6.0), tests de `workers/vision`, `scripts/acceptance.ps1 -WithClaude -RequireVision` (25 PASS, 0 FAIL, 0 SKIP, con Edge real y `claude -p` real) y una prueba real de `claude -p` con los hooks de `kepler hook claude-code` (lectura de `.env` bloqueada, lectura inocua permitida, eventos registrados sin el secreto). **No probado:** Linux y macOS, Chromium descargado por Playwright (solo Edge), modelo remoto real con Pi, TUI interactivo (Pi y Claude Code), los workflows nuevos de GitHub Actions (`acceptance.yml`, etc.), instalación desde un Release (no existe ninguno).
* Verificado con un Pi real 1.6.0 (e2e: `npm run test:e2e` en `adapters/pi`, modelo scripted sin credenciales; ver [NPM_AND_PI](docs/distribution/NPM_AND_PI.md)). **No** verificado: aprobaciones interactivas en el TUI de Pi, avisos de Focus en un TUI real, un modelo remoto real, cliente MCP de terceros, Chromium real, Linux y macOS.
* Benchmark A/B con/sin Kepler con un modelo real (KPL-032); compresión de salida de herramientas; detección/instalación de harnesses; primer release real y paquete npm (el adaptador Pi sigue `private: true` hasta aprobar propiedad del scope npm y release).
* Flujo de diseño/generación de UI de Genesis (ADR-0018); `kepler ui` es solo un panel.

## Documentación

[Visión](docs/VISION.md) · [Arquitectura](docs/architecture/OVERVIEW.md) · [Eventos y capacidades](docs/architecture/EVENTS_AND_CAPABILITIES.md) · [Adaptadores](docs/architecture/HARNESSES.md) · [Focus](docs/architecture/FOCUS.md) · [Módulos](docs/modules/README.md) · [Genesis](docs/modules/GENESIS.md) · [Modelo de amenazas](docs/security/THREAT_MODEL.md) · [Roadmap](docs/roadmap/MVP.md) · [Backlog y estado](docs/roadmap/BACKLOG.md) · [Decisiones (ADR)](docs/decisions/README.md) · [Guía de desarrollo](docs/development/GETTING_STARTED.md) · [Rendimiento](docs/development/PERFORMANCE.md) · [Benchmarks](docs/quality/BENCHMARKS.md) · [Releases](RELEASING.md) · [Cambios](CHANGELOG.md) · [Contribuir](CONTRIBUTING.md)

## Licencia

[MIT](LICENSE) (2026). Los crates declaran `license = "MIT"` y siguen con `publish = false` (no se publican en crates.io); `adapters/pi` y `workers/vision` declaran MIT pero permanecen `private: true`.
