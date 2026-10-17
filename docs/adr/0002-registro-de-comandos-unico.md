# ADR 0002: Registro de comandos único como fuente de la UI, la CLI, el MCP, el agente y Python

- Estado: aceptada (Hito 1)
- Fecha: 2026-10-16
- Referencias: `docs/03-architecture.md` §4, `docs/07-mcp-ai-architecture.md` §4

## Contexto

Si cada fachada (MCP, CLI, UI, agente integrado) declara a mano sus operaciones, los nombres, los argumentos y las reglas divergen. Además, los agentes necesitan schemas exactos y estables, y las políticas de seguridad (aprobaciones, solo lectura) no pueden depender de que cada cliente las respete.

## Decisión

- Cada operación del editor es un `CommandSpec` declarado una vez en `forma-engine` (`registry()`): nombre, título, descripción, conjunto (`toolset`), efectos (`read_only`, `destructive`, `idempotent`), tipo de resultado, schema de entrada, schema de salida y manejador.
- Los schemas se **generan** desde los tipos Rust con `schemars`; no se escribe ninguno a mano.
- Hay 35 herramientas, con nombres `snake_case` de hasta 40 caracteres y un orden de registro determinista, repartidas en ocho conjuntos (`core`, `glyphs`, `geometry`, `spacing`, `quality`, `export`, `preview`, `history`).
- Toda llamada pasa por `Engine::call(actor, tool, args)`, que valida los argumentos contra el schema (campos desconocidos incluidos), aplica los límites, el modo solo lectura, los conjuntos habilitados, la caja de rutas y las aprobaciones, y registra la auditoría. Las fachadas no filtran nada por su cuenta: `Engine::enabled_specs()` dice qué herramientas existen.
- El servidor MCP se genera del registro en tiempo de ejecución; la CLI expone cada herramienta con `forma call <herramienta>` y añade subcomandos cómodos (`new`, `info`, `export`, `validate`, `pack`, `unpack`) que solo encadenan llamadas al motor.
- La documentación de la API (`docs/api/tools.md`) se genera del registro con `forma gen-docs`, y el test `api_docs_up_to_date` falla si el archivo no coincide.
- Pruebas de paridad: el test `parity_every_registry_tool_is_callable` recorre las 35 herramientas desde la CLI, y el snapshot de `tools/list` congela el contrato MCP (un cambio debe justificarse como aditivo o incompatible; un cambio incompatible es una herramienta nueva con sufijo `_v2`).

## Alternativas

- **Declarar las herramientas en cada fachada:** más simple al inicio, pero garantiza divergencias y duplica la validación.
- **Macros `#[tool]` de rmcp:** acoplan el registro al SDK de MCP; la UI, la CLI y el agente no podrían derivar de él.
- **Schemas escritos a mano:** se desincronizan de los tipos.

## Consecuencias

- Añadir una operación toca un solo lugar y aparece en MCP, CLI y documentación; olvidarlo en una fachada es imposible por construcción.
- El registro es API pública: renombrar o cambiar un schema rompe el snapshot y exige una decisión consciente.
- El tamaño de `tools/list` crece con cada herramienta (hoy unos 185 KB compactos, el 60 % en schemas de salida); optimizarlo (definiciones compartidas, conjuntos por defecto) queda para más adelante.
- Las anotaciones MCP (`readOnlyHint`, `destructiveHint`, `idempotentHint`) derivan de `effects`, pero son solo pistas para el cliente: la aplicación real está en el motor.
