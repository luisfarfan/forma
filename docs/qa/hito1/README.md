# QA Hito 1 — Demostración con agentes reales (criterio 10)

> Fecha: 2026-10-16 · Máquina: macOS 27.0 (Apple Silicon) · `forma` 0.0.1 instalado con `cargo install --path core/crates/forma-cli --locked` desde la rama `hito1`.
> Esta demostración es manual y no bloquea la CI. Los criterios automatizados del hito están en `docs/04-roadmap.md` y se verifican con `scripts/acceptance.sh`.

## Guion

Cada agente se conectó a `forma mcp` por stdio, sin interfaz gráfica:
- `--allow-dir` limitado a su carpeta (`claude/` o `codex/`);
- `--journal-dir` temporal.

Ambos recibieron la misma instrucción en lenguaje natural:

> *Using only the forma MCP tools: create a font document called 'Forma Demo' saved at …/forma-demo-agent.forma. Draw the glyphs H, O, n and o (simple geometric sans-serif shapes, units per em 1000, cap height 700, x-height 500) and a space glyph, with reasonable spacing. Check the geometry and fix any fixable issue, validate the font until it has zero errors, export a TTF to …/forma-demo-agent.ttf, save the document, and show a preview of 'o'. Finish with a short summary of what you did and the final validation counts.*

## Resultados

| | Claude Code 2.1.302 (`claude -p`) | Codex CLI 0.160.0 (`codex exec`) |
|---|---|---|
| Era del protocolo MCP | La que negocia el cliente | La que negocia el cliente (por defecto 2025-06-18) |
| Llamadas a herramientas FORMA | 13 (0 errores) | 10 (0 errores) en la ejecución 2 |
| Flujo | capabilities → create → inspect → **edit_batch** (5 glifos) → metrics_set → geometry_check → validate → metrics_get → preview_text → export_font → save → preview_glyph | capabilities → create → **edit_batch** → geometry_check → metrics_get → validate → export_font → save → preview_glyph → inspect |
| `validate` final | 0 errores, 0 avisos | 0 errores, 0 avisos |
| TTF exportado | `claude/forma-demo-agent.ttf` (1 176 bytes) | `codex/forma-demo-agent.ttf` (1 168 bytes) |
| OTS (`ots-sanitize`) | OK | OK |
| fontTools: familia y cmap | "Forma Demo"; `.notdef H O n o space`; cmap correcto | Igual |
| HarfBuzz `hb-shape "HOno"` | Glifos y avances del modelo (680, 756, 562, 574) | Correcto |
| fontspector (perfil opentype) | 0 FAIL · 2 WARN (`no-stat-table`, `no-wws-without-wws-names`: esperados en una fuente demo estática) | 0 FAIL · 2 WARN (los mismos) |
| Duración / costo | 98 s · 0,74 USD reportados por el cliente | Una sola vuelta del agente; ≈ 340 k tokens de entrada (90 % en caché) |

Vistas previas generadas con `forma call preview_text` sobre los `.forma` guardados por cada agente:

- Claude Code: `claude/preview-HOno.png`
- Codex: `codex/preview-HOno.png`

Las transcripciones condensadas (llamadas, argumentos resumidos, resultados y mensaje final) están en `claude-code-transcript.md` y `codex-transcript.md`. Los JSONL crudos no se versionan porque contienen metadatos del entorno local.

## Hallazgos

1. **El servidor se comportó correctamente con ambos clientes y ambos modos de ciclo de vida.** Hubo 0 errores del servidor; ningún agente necesitó reintentos por conflictos de revisión.
2. **Codex y las anotaciones `destructiveHint`.** En la primera ejecución de Codex (`approval_policy = "never"`, `default_tools_approval_mode = "auto"`), el *cliente* exigió aprobación humana para las herramientas anotadas como destructivas y, sin humano, las rechazó ("MCP tool call requires approval, but approval policy is never"). Las afectadas fueron `edit_batch`, `export_font` y `document_save`.
   - El agente rodeó `edit_batch` creando los glifos uno a uno, pero no pudo exportar ni guardar.
   - La segunda ejecución, con las herramientas FORMA preaprobadas (`default_tools_approval_mode = "approve"`), completó todo.
   - En uso interactivo el usuario aprueba esas llamadas.
   - **Pendiente para el próximo hito:** `document_save`, `export_font` y `edit_batch` solo son destructivas en casos que el servidor ya protege con planes. Conviene revaluar su anotación (hoy `destructive: true`) para reducir la fricción en Codex sin perder seguridad, porque la seguridad la impone el servidor.
3. **Ambos agentes usaron `edit_batch` para dibujar todos los glifos en una sola transacción deshacible**, y consultaron `geometry_check` y `validate` antes de exportar, tal como describen las instrucciones del servidor.
4. Ninguna operación requirió plan de aprobación: no hubo borrados ni cambios masivos.

## Cómo repetirla

Ver `docs/guides/connect-agents.md`. Comandos usados (rutas abreviadas):

```bash
# Claude Code (headless)
claude -p "<instrucción>" --mcp-config mcp.json --strict-mcp-config \
  --allowedTools "mcp__forma" --output-format stream-json --verbose

# Codex (no interactivo; shell en solo lectura, herramientas FORMA preaprobadas)
codex exec --skip-git-repo-check --ephemeral --json -s read-only \
  -c approval_policy='"never"' \
  -c 'mcp_servers.forma.command="forma"' \
  -c 'mcp_servers.forma.args=["mcp","--allow-dir","<dir>","--journal-dir","<tmp>"]' \
  -c 'mcp_servers.forma.default_tools_approval_mode="approve"' \
  "<instrucción>"
```
