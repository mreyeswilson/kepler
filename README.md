# Kepler

**Intelligent Execution Control for Coding Agents** — runtime local-first que reduce desvíos, trabajo innecesario, afirmaciones sin evidencia y tiempos excesivos.

> Estado: núcleo Rust con Forge, Memory y Guardian, servidor MCP mínimo y adaptador Pi con hooks automáticos, todo en desarrollo y **sin verificar contra una sesión real de Pi**; ver "Estado de implementación".

## Objetivo

Kepler mejora la experiencia de desarrollo con Pi, Claude Code, Codex, Gemini CLI, OpenCode y otros harnesses compatibles. La automatización ocurre a través de eventos nativos, hooks y adaptadores; MCP y CLI sirven como interfaces interoperables. Las capacidades automáticas varían según el harness.

## Control central y siete módulos

| Módulo | Responsabilidad |
|---|---|
| **Focus (central)** | Protege objetivo, alcance, progreso, evidencia y presupuesto de ejecución |
| **Forge** | Compilar instrucciones adaptativamente conservando la intención original |
| **Memory** | Recuperar y consolidar memoria relevante con procedencia |
| **Guardian** | Optimizar contexto, presupuestos y métricas de tokens |
| **Orbit** | Coordinar tareas y agentes con límites y aislamiento |
| **Sentinel** | Auditar cambios, riesgos, pruebas y seguridad |
| **Vision** | Inspeccionar UI bajo demanda mediante Playwright |
| **Genesis** | Diseñar interfaces, generar variaciones, componentes y código con validación posterior |

## Arquitectura de referencia

- **Core:** Rust estable, Tokio y módulos compilados.
- **Persistencia:** SQLite + WAL + FTS5; sin dependencia de nube.
- **Integración:** CLI, MCP por stdio, adaptadores con negociación de capacidades y hooks nativos.
- **Ejecución:** one-shot CLI + daemon local compartido y bajo demanda.
- **Análisis:** Git, Tree-sitter y LSP opcionales.
- **UI:** Playwright con worker aislado en TypeScript.
- **Seguridad:** mínimos privilegios, filtrado de secretos, límites de recursos y confirmación antes de acciones destructivas.

## Principios

1. **Automático, pero proporcional:** no ejecutar análisis ni modelos cuando no aportan valor.
2. **Portable, no idéntico:** cada harness expone capacidades distintas; nunca fingir integración profunda vía MCP.
3. **Local-first y eficiente:** nada pesado en reposo; workers y modelos bajo demanda.
4. **Conservar la intención:** distinguir el texto original del usuario de inferencias de Forge.
5. **Memoria selectiva:** recordar decisiones duraderas, no ruido o secretos.
6. **Fail-open para mejoras opcionales, fail-closed para operaciones peligrosas.**
7. **Observable y reversible:** cada intervención crítica queda trazada y puede deshabilitarse.

## Documentación

- [Visión](docs/VISION.md)
- [Arquitectura](docs/architecture/OVERVIEW.md)
- [Eventos y capacidades](docs/architecture/EVENTS_AND_CAPABILITIES.md)
- [Adaptadores](docs/architecture/HARNESSES.md)
- [Detección de CLI y clientes de escritorio](docs/architecture/DISCOVERY.md)
- [Persistencia](docs/architecture/STORAGE.md)
- [Módulos](docs/modules/README.md)
- [Control de ejecución Focus](docs/architecture/FOCUS.md)
- [Diseño de interfaces Genesis](docs/modules/GENESIS.md)
- [Benchmark y métricas](docs/quality/BENCHMARKS.md)
- [Modelo de amenazas](docs/security/THREAT_MODEL.md)
- [Roadmap MVP](docs/roadmap/MVP.md)
- [Backlog](docs/roadmap/BACKLOG.md)
- [Decisiones de arquitectura](docs/decisions/README.md)
- [Guía de desarrollo](docs/development/GETTING_STARTED.md)
- [Instrucciones para agentes](AGENTS.md)
- [Distribución npm y Pi](docs/distribution/NPM_AND_PI.md)
- [Paquete Pi (esqueleto)](adapters/pi/README.md)

## Desarrollo

### Estado de implementación (Deliveries 0-2, rama `feat/rust-core-d0`)

Existe un workspace Rust (`crates/`) con tests. **Implementado y probado:** envelope de eventos v1 con fixtures compartidos, negociación de las 8 capacidades, motor de políticas determinista (passthrough por defecto, preflight de herramientas fail-closed), IPC local (Unix socket / named pipe del usuario actual, ver [ADR-0007](docs/decisions/0007-ipc-wire-protocol.md)), daemon lazy con apagado por inactividad, SQLite (WAL, FTS5, migraciones) con redacción de secretos y denylist de rutas, y los comandos `kepler doctor | daemon | emit | version`.

**Delivery 1-2 (implementado y probado con tests; ver [backlog](docs/roadmap/BACKLOG.md) para el estado exacto de cada ID):** Forge (clasificador determinista passthrough/enrich/compile/decompose y spec derivada, [ADR-0009](docs/decisions/0009-module-pipeline-and-forge.md)), Memory (FTS5 por alcance, ranking, dedupe, presupuesto de tokens estimados, consolidación al asentarse el agente, controles `kepler memory ...`, [ADR-0010](docs/decisions/0010-memory-storage-and-ranking.md)), Guardian (preflight fail-closed con reglas del usuario, detección de secretos en resultados, presupuesto de contexto, [ADR-0011](docs/decisions/0011-guardian-and-wire-additions.md)), `kepler mcp serve` ([ADR-0012](docs/decisions/0012-mcp-stdio-minimal-server.md)) y `kepler inspect`, y hooks automáticos en `adapters/pi` ([ADR-0013](docs/decisions/0013-pi-automatic-hooks.md)). Mediciones reales en [PERFORMANCE.md](docs/development/PERFORMANCE.md).

**No implementado todavía:** Focus, Orbit, Sentinel, Vision, Genesis, detección de harnesses, binarios/instalación, compresión de salida de herramientas, verificación con una sesión real de Pi y con un cliente MCP de terceros. La licencia sigue pendiente (`publish = false`, `UNLICENSED` en Cargo). El código Unix del transporte aún no se ha ejecutado fuera de Windows; lo cubrirá la matriz de CI.

```bash
cargo build --workspace
cargo run -p kepler-cli -- doctor        # diagnóstico de solo lectura
cargo run -p kepler-cli -- emit --in-process --json '{...evento v1...}'
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
```

Los siguientes comandos siguen siendo **propuestas** sin implementar: `kepler init`, `kepler detect`. Ya existen `doctor | daemon | emit | version | inspect | memory | mcp serve`.

El paquete Pi está en `adapters/pi` (marcado `private: true`, **sin publicación npm; hooks implementados pero no verificados en Pi real**). La futura instalación pública será `pi install npm:@kepler/pi`, sujeta a disponibilidad del nombre y pruebas de publicación.

Se contempla detección de Pi, Claude Code, Claude Desktop, Codex, Gemini CLI, OpenCode, Cursor, VS Code/Copilot y otros clientes MCP. **La detección y la instalación automática son requisitos futuros; aún no están implementados.**

**Primera integración objetivo:** Pi Coding Agent. **Primera entrega funcional:** núcleo de eventos + adaptador Pi + Focus mínimo + Forge y Memory. Genesis se implementará después del núcleo de control y la validación visual.

## Licencia

Pendiente de elegir. No asumir que el repositorio tiene una licencia open source concedida hasta añadir un archivo `LICENSE`.
