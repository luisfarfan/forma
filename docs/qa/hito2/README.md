# QA Hito 2 — Demostración con un agente real (criterio 8)

> Fecha: 2026-10-17 · Máquina: macOS 27.0 (Apple Silicon) · `forma` 0.0.1 instalado con `cargo install --path core/crates/forma-cli --locked` desde la rama `hito2`.
> La demostración es manual y no bloquea la CI. Los criterios automatizados del hito están en `docs/04-roadmap.md` y se verifican con `scripts/acceptance.sh`.
> Las decisiones del controlador y lo diferido están en [`decisiones.md`](decisiones.md).
> **Aquí solo se publica la demo con la imagen de prueba propia** (`tests/fixtures/hito2/specimen.png`, generada por código). La imagen del alfabeto de la hermana del responsable y la fuente que salga de ella se quedan en local, fuera del repositorio.

## Guion

Claude Code 2.1.302 (`claude -p`) se conectó a `forma mcp` por stdio, sin interfaz gráfica y sin otras herramientas:
- `--allow-dir` limitado a la carpeta de trabajo de la demo;
- `--journal-dir` temporal;
- `--allowedTools mcp__forma`.

La imagen de entrada fue `specimen.png`: 8 letras propias (A B C H / O F M R) en naranja sobre fondo oscuro, con un círculo azul que distrae.

La instrucción fue:

> *Using only the forma MCP tools, turn the alphabet in specimen.png into a complete font called 'Specimen Demo' …: trace every letter (they are A B C H in the first row and O F M R in the second; map each to its uppercase and lowercase code points), design the digit 0 and the acute accent mark (acutecomb) in the same style using the measured stroke, compose Á and Ó, space and kern the font, check it has no collisions, validate it with zero errors, export TTF … and WOFF2 …, save it, and show me a preview of 'HOLA ÁRBOL 0'. If a tool asks for approval through a plan, commit the plan yourself: I approve all plans in advance for this demo.*

## Resultados

| | Claude Code 2.1.302 |
|---|---|
| Llamadas a herramientas FORMA | 40, sin errores de servidor. Hubo un `APPROVAL_REQUIRED` esperado y un `plan_commit` bloqueado por el cliente; ver Hallazgos. |
| Flujo | `forma_capabilities` → `image_inspect` → `document_create` → `image_trace` (8 letras) → `geometry_check` → `glyph_measure` (×4) → `glyph_create` + `glyph_stroke` (0 y acutecomb) → `remove_overlap` → `geometry_fix` → `font_set_vertical_metrics` → `glyph_compose` (Á, Ó) → `spacing_auto` → `kerning_auto` → `kerning_check` (×2) → `validate` → `charset_check` → `preview_text` → `export_font` (TTF, WOFF2) → `document_save` |
| Fidelidad del trazado (IoU) | A 0,997 · B 0,996 · C 0,987 · H 1,000 · O 0,991 · F 1,000 · M 0,999 · R 0,998 |
| Estilo medido | Trazo de unas 80 unidades con terminales redondos. El 0 y la tilde se dibujaron con ese trazo; el 0 mide 79,5. |
| Espaciado y kerning | 9 glifos espaciados, ninguno con margen recortado a 0. 8 pares de kerning sobre 6 grupos. |
| Colisiones | 0 en los 121 pares de letras, 0 y acentuadas, a 20 unidades de distancia mínima. |
| `validate` | 0 errores, 0 avisos |
| Cobertura (`charset_check`) | 22 de 22 códigos pedidos |
| TTF y WOFF2 | `claude/specimen-demo.ttf` (2 452 bytes) y `claude/specimen-demo.woff2` (1 500 bytes) |
| OTS | OK en los dos |
| fontTools | 14 glifos; `Aacute` y `Oacute` compuestos en `glyf`; tabla GPOS presente; cmap con las 8 letras en mayúscula y minúscula, Á á Ó ó, 0 y espacio |
| fontspector (perfil `opentype`) | 0 FAIL |
| Duración y costo | 153 s · 43 turnos · 1,34 USD, según el cliente |

![HOMBRO FÁRMACO 0 compuesto con la fuente de la demo](claude/preview.png)

Este preview se generó con `forma call preview_text` sobre el `.forma` guardado. Las cajas son espacios: el documento de trabajo no tiene glifo `space`, que se añade al exportar (ver Hallazgos 3). Es el registro de la demo tal como ocurrió, antes de la ola final; hoy el mismo comando deja ahí huecos en blanco.

## Hallazgos

1. **La regla de aprobación funcionó como se diseñó.** El agente intentó borrar sus Á/Ó con `glyph_delete`, y el servidor respondió con un plan (`APPROVAL_REQUIRED`). Claude Code bloqueó después el `plan_commit` ("MCPTool requires permission"), aun con `--allowedTools mcp__forma`. El motivo es que esa herramienta declara que necesita interacción humana (`anthropic/requiresUserInteraction`). Así, un agente no puede aprobar solo sus propias operaciones destructivas, aunque el prompt diga "apruebo todo".
   - El agente no insistió: renombró las versiones malas, las marcó como no exportables y compuso otras nuevas.
2. **Defecto encontrado: los compuestos no se realinean al reespaciar la base.** El agente compuso Á y Ó antes de espaciar. Al cambiar los márgenes de la A y la O, las compuestas conservaron el avance viejo y la tilde quedó descentrada.
   - La revisión final lo confirmó en la fuente `unicase-es` de aceptación: todas las acentuadas con avance viejo (Aacute 740 frente a A 674) y la marca descentrada entre −60 y −150 unidades; además, `spacing_auto` espaciaba las marcas de avance 0.
   - **Corregido en la ola final:** `spacing_auto` realinea en la misma transacción cada compuesto cuya base reespacia (mismo cambio de avance; las marcas se mueven con la base; campo `realigned`) y no toca las marcas de avance 0. El flujo de las instrucciones ahora compone antes de espaciar. Medido con fontTools en las dos fuentes de aceptación regeneradas: las 9 acentuadas tienen el avance de su base y la marca centrada (desvío 0,0). Véase `decisiones.md`, decisiones 22–24.
3. **Mejora: `preview_text` muestra una caja para el espacio cuando el documento no tiene glifo `space`.** La exportación ya lo añade solo. **Corregido en la ola final:** sin glifo para U+0020 o U+00A0, el preview deja un hueco en blanco de `round(UPM / 4)`, el ancho del `space` que añade la exportación (decisión 26).
4. **`kerning_check` sobre todos los glifos reporta una colisión entre dos `acutecomb`.** Es lo esperado: una marca combinante de avance 0 se monta sobre la letra anterior. **Corregido en la ola final:** el conjunto por defecto de `kerning_auto` y `kerning_check` excluye las marcas de avance 0 (se pueden incluir con `glyphs`; decisión 23).
5. Codex no se ejecutó en esta ronda. La compatibilidad de la era del protocolo que usa Codex se cubre con el escenario MCP automatizado en modo `initialize` (`mcp_scenario_hito2.rs`).

## Cómo repetirla

Ver `docs/guides/connect-agents.md` (sección "De una imagen a una fuente"). El comando usado, con rutas abreviadas:

```bash
claude -p "<instrucción>" --mcp-config mcp.json --strict-mcp-config \
  --allowedTools "mcp__forma" --output-format stream-json --verbose
```

con `mcp.json`:

```json
{"mcpServers":{"forma":{"command":"forma","args":["mcp","--allow-dir","<carpeta>","--journal-dir","<tmp>"]}}}
```
