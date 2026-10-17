# ADR 0005: Servidor MCP con rmcp por stdio, en ambas eras del protocolo, con planes y tokens de aprobación

- Estado: aceptada (Hito 1)
- Fecha: 2026-10-16
- Referencias: `docs/07-mcp-ai-architecture.md` §2, §3, §7, §10 y §12; `docs/guides/connect-agents.md`

## Contexto

Claude Code y Codex lanzan servidores MCP locales por stdio, pero hablan versiones distintas del protocolo: Codex usa el ciclo `initialize` (2025-06-18) y la revisión 2026-07-28 reemplaza el apretón de manos por `server/discover`. Codex además descarta las imágenes de una herramienta si la respuesta lleva `structuredContent`. Los agentes pueden equivocarse o ser engañados por el contenido de un documento, así que las operaciones destructivas o masivas no pueden depender de que el cliente pida confirmación.

## Decisión

- **SDK y transporte.** `forma-mcp` usa `rmcp` 3.5 (SDK oficial, sin la característica de macros) sobre **stdio**. Por stdout solo pasa JSON-RPC; los logs van a stderr (`FORMA_LOG`). `forma mcp` es un subcomando del binario `forma`.
- **Ambas eras, con mínimo común denominador.** Se aceptan todas las versiones que rmcp soporta del ciclo `initialize` y la era `server/discover` de 2026-07-28, donde cada petición lleva su `_meta`. Solo se anuncian herramientas (`listChanged: false`) y recursos básicos (`forma://docs`, `forma://doc/{doc}/glyph/{nombre}` y su `preview.png`); no se usan elicitación, sampling ni Tasks. Las pruebas de integración corren con el cliente de rmcp en los dos modos.
- **Resultados.** Las herramientas de datos devuelven `structuredContent` y el mismo JSON como texto. Las de preview devuelven imagen PNG más un texto con el resumen, **sin `structuredContent`**. Un error es un resultado `isError` con `{code, message, details}` y uno de once códigos estables.
- **Actor.** Cada llamada se hace como `agent:<clientInfo.name>` (sin caracteres de control, máximo 64 caracteres) o `agent:unknown`.
- **Todo se aplica en el servidor.** Las anotaciones (`readOnlyHint`, `destructiveHint`, `idempotentHint`) son pistas derivadas de los efectos del registro: `destructiveHint` es verdadero en toda herramienta que *puede* exigir aprobación. La aplicación real está en el motor: límites por llamada (2 000 operaciones, 100 000 nodos, 4 MiB por texto), conjuntos de herramientas, `--read-only`, caja de rutas `--allow-dir` y registro de auditoría.
- **Plan → token → commit.** Una operación destructiva (entre otras: `glyph_delete`; `glyph_set_outline` sobre una capa con contornos; quitar el último contorno de un glifo; `history_restore`; `history_revert` que borra glifos; `document_save` o `export_font` sobre un archivo existente; cerrar con `discard_changes` un documento con cambios; un `edit_batch` que contenga alguna) o masiva (más de 20 glifos) devuelve `APPROVAL_REQUIRED` con un plan: resumen, glifos afectados, diff acotado y `plan_token`.
  - El token es `base64url(plan_id ‖ HMAC-SHA256(clave, plan_id ‖ actor ‖ herramienta ‖ SHA-256(argumentos canónicos) ‖ base_rev ‖ caducidad))`. La clave son 32 bytes aleatorios generados al crear el motor; el `plan_id`, 16 bytes aleatorios. El servidor guarda además el plan completo.
  - `plan_commit` verifica la firma con comparación en tiempo constante, que el actor sea el mismo y que no haya caducado (300 s), **elimina el plan antes de ejecutar** (uso único, aunque falle) y ejecuta la herramienta guardada con sus argumentos y el `base_rev` del plan, de modo que `REVISION_CONFLICT` sigue protegiendo contra cambios posteriores al plan. Los planes de escritura de archivos (`document_save`, `export_font`) fijan además la revisión.
  - `dry_run: true` devuelve un plan sin cambiar nada; `plan_discard` lo descarta. Los planes caducados se purgan, hay un máximo de 1 000 planes y 64 MiB, y los de un documento cerrado se eliminan.
  - Políticas: `prompted` (por defecto), `auto` (las masivas no destructivas se aplican) y `strict`, que responde `UNSUPPORTED` a toda operación que exija aprobación hasta que la aplicación pueda aprobar (Hito 3).
  - `plan_commit` lleva `_meta` `anthropic/requiresUserInteraction`, para que los clientes que lo entiendan pidan confirmación humana.
- **Seguridad del contenido.** Los nombres de glifos y demás datos de un documento son datos no confiables y se devuelven como datos; no hay herramientas que ejecuten código ni lean fuera de las raíces permitidas.
- **Contrato.** El snapshot de `tools/list` (insta) congela las 35 herramientas; cambiarlo exige explicar si el cambio es aditivo o incompatible.

## Alternativas

- **Otro SDK (`pmcp`):** menos adopción que el SDK oficial; rmcp ya cubre ambas eras.
- **Aprobar con elicitación MCP:** no todos los clientes la soportan; el plan con token funciona con cualquiera.
- **Confiar en las anotaciones o en la confirmación del cliente:** un cliente mal configurado o un prompt inyectado se saltaría la protección.
- **Tokens sin firma, solo identificadores guardados en el servidor:** el HMAC liga el plan al actor, a la operación y a la revisión sin depender de que el almacén no se manipule.
- **Un servidor por era del protocolo:** doble mantenimiento para las mismas herramientas.

## Consecuencias

- Cualquier cliente MCP corriente sirve; añadir uno nuevo no cambia el servidor.
- `tools/list` es grande (unos 185 KB compactos; un 60 % son schemas de salida, que los clientes no necesitan para llamar): conviene reducirlo con definiciones compartidas o conjuntos por defecto. El MCP Inspector, además, avisa de que los `type: ["string", "null"]` de los schemas son poco portables a algunos proveedores.
- Los tokens viven y mueren con el proceso (la clave es efímera): un plan mostrado por `forma call` sin `--yes` es solo informativo, y reiniciar el servidor invalida los planes pendientes.
- La cancelación de una llamada larga no se propaga al motor (la llamada termina y su resultado se descarta).
- El flujo `strict` queda sin implementar hasta que exista la interfaz que lo apruebe.
