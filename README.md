# Kepler

**Universal AI Agent Runtime** — una capa de inteligencia local-first, automática y multi-harness para coding agents.

> Estado: diseño y documentación inicial. Sin implementación funcional.

## Objetivo

Kepler mejora la experiencia de desarrollo con Pi, Claude Code, Codex, Gemini CLI, OpenCode y otros harnesses compatibles. La automatización ocurre a través de eventos nativos, hooks y adaptadores; MCP y CLI sirven como interfaces interoperables. Las capacidades automáticas varían según el harness.

## Seis módulos

| Módulo | Responsabilidad |
|---|---|
| **Forge** | Compilar instrucciones adaptativamente conservando la intención original |
| **Memory** | Recuperar y consolidar memoria relevante con procedencia |
| **Guardian** | Optimizar contexto, presupuestos y métricas de tokens |
| **Orbit** | Coordinar tareas y agentes con límites y aislamiento |
| **Sentinel** | Auditar cambios, riesgos, pruebas y seguridad |
| **Vision** | Inspeccionar UI bajo demanda mediante Playwright |

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
- [Persistencia](docs/architecture/STORAGE.md)
- [Módulos](docs/modules/README.md)
- [Modelo de amenazas](docs/security/THREAT_MODEL.md)
- [Roadmap MVP](docs/roadmap/MVP.md)
- [Backlog](docs/roadmap/BACKLOG.md)
- [Decisiones de arquitectura](docs/decisions/README.md)
- [Guía de desarrollo](docs/development/GETTING_STARTED.md)
- [Instrucciones para agentes](AGENTS.md)
- [Distribución npm y Pi](docs/distribution/NPM_AND_PI.md)
- [Paquete Pi (esqueleto)](adapters/pi/README.md)

## Desarrollo

El proyecto está en fase de especificación. Los ejemplos de comandos y nombres de paquetes son **propuestas**, no herramientas publicadas ni ejecutables:

```bash
kepler init
kepler doctor
kepler mcp serve
```

El esqueleto del paquete Pi está en `adapters/pi` (marcado `private: true`, **sin publicación npm ni hooks funcionales**). La futura instalación pública será `pi install npm:@kepler/pi`, sujeta a disponibilidad del nombre y pruebas de publicación.

**Primera integración objetivo:** Pi Coding Agent. **Primera entrega funcional:** núcleo de eventos + adaptador Pi + Forge y Memory.

## Licencia

Pendiente de elegir. No asumir que el repositorio tiene una licencia open source concedida hasta añadir un archivo `LICENSE`.
