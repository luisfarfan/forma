# Hito 2.5 — Registro de decisiones

Decisiones tomadas durante la implementación del Hito 2.5 que la especificación no fijaba, o que la ajustan. Cada entrada indica el motivo y el costo si resultara equivocada. Las decisiones del responsable están en §11 de la especificación.

## Contrato `tools/list`

| Cambio | Clase | Motivo |
|---|---|---|
| `glyph_review` (nueva, conjunto `glyphs`, `WRITE`) | Aditivo | Revisión de glifos generados (§5.4) |
| `provenance` (opcional) en el schema de `Glyph` | Aditivo | Procedencia en el documento (ADR 0010) |
| `set_provenance` en el enum de comandos | Aditivo, interno | Lo emiten `image_trace`, `glyph_compose` y `glyph_review`; `edit_batch` no lo expone |
| `detail` (opcional) en `spacing_auto`, `kerning_auto`, `kerning_get`, `kerning_check`, `geometry_check`, `geometry_fix`, `validate`, `image_trace`, `charset_check` | Aditivo en el schema; **cambio de comportamiento compatible** | Por defecto `summary`: listas de hasta 20 entradas / 4 KB (antes 200 / 32 KB), `touched` hasta 20 y `validate` sin hallazgos `info`. `full` restaura las listas largas. El detalle completo va al informe (`report_path`) |
| `report_path` / `report_note` (opcionales) en las salidas de esas herramientas | Aditivo | Informes en archivo (§5.4) |
| `iou_stats`, `traced_total` en `image_trace` | Aditivo | Resumen sin listar todos los glifos |
| `source`, `review` (opcionales) en `glyph_list`; `provenance` en `glyph_get` | Aditivo | Encontrar glifos pendientes sin leerlos uno a uno |
| `kerning_auto` devuelve `OrPlan<ReportedMutation>` | Aditivo (mismos campos + `report_path`) | — |
| Texto de `touched` y de la descripción de `validate` | Documentación | Refleja los límites de `summary` |

## Decisiones

1. **Formato v3 sin cambios de contenido en la migración.** `provenance` es opcional, así que la migración v1/v2 → v3 es la identidad sobre el JSON. El respaldo `<archivo>.v<n>.bak` se escribe en `forma_store::save` antes de reemplazar un documento más antiguo, una sola vez. *Costo si es incorrecto:* un respaldo de más en disco por documento migrado.
2. **"Cambio de diseño" = contorno expandido (componentes resueltos), normalizado horizontalmente y cuantizado a 0,001 unidades, comparado una vez por transacción** (`forma_geometry::same_design`, `forma_edit::mark_design_changes`). Mover todo el contorno de lado (lo que hacen `lsb` y `spacing_auto`, incluida la recolocación de compuestos) no invalida una aprobación; mover una marca respecto de su base o un nodo, sí. Cambiar el indicador `smooth` cuenta como cambio de diseño (conservador). *Corregido tras la revisión del bloque A:* la primera versión comparaba comando a comando y los desplazamientos crudos de los componentes, así que el reespaciado devolvía los compuestos aprobados a revisión.
3. **`glyph_review` devuelve el `MutationResult` simple** (`touched_count` da el número de glifos revisados), en lugar de un campo `reviewed` propio. *Costo:* ninguno funcional.
4. **`thresholds_version = 0` en la procedencia de `image_trace` y `glyph_compose`** hasta que existan los umbrales versionados (Task 12); entonces pasa a la versión vigente.
5. **La cabecera del informe registra la revisión resultante de la llamada** (`rev` después de un cambio), no la `base_rev`: es el estado que el informe describe. El nombre de archivo usa esa misma revisión.
6. **`image_trace` en `summary` conserva el orden de las asignaciones**: si hay más de 20, lista las de menor IoU, en su orden original (`traced_total` e `iou_stats` resumen el resto).
7. **Perfil `image-font`:** lista fija en `forma_engine::registry::PROFILES`; el motor rechaza (`UNSUPPORTED`) lo que no está en el perfil, igual que con `--toolsets`. Los nombres aún no registrados (`preview_sheet`, `font_from_image`, `glyph_synthesize`) se ignoran hasta que existan.
8. **`--output-schemas none`** solo cambia la presentación (fachada MCP): los resultados siguen trayendo `structuredContent`.

## Tamaño del catálogo (`forma mcp --print-catalog-size`, estimación, no medida por cliente)

| Configuración | Herramientas | Caracteres | Tokens estimados (÷4) |
|---|---|---|---|
| Por defecto (Hito 2.5, bloque A) | 51 | 337 363 | ~84 k |
| `--profile image-font --output-schemas none` | 23 | 48 367 | ~12 k |
9. **Un rechazo se mantiene tras una edición geométrica** (solo `approved` vuelve a `pending`; `edited = true` en ambos casos). Un glifo rechazado sigue fuera de la exportación hasta que alguien lo revise de nuevo. *Corregido tras la revisión del bloque A.*
10. **El respaldo nunca se omite en silencio:** si hay un documento en la ruta y no se puede leer su versión (bloqueo, permisos, cabecera dañada), `save` falla con `Io` y no reemplaza nada. Una ruta de paquete con separador final deja el respaldo como hermano, no dentro del paquete. Los archivos copiados al respaldo se sincronizan a disco.
11. **Informes solo dentro de las raíces permitidas:** si la carpeta del documento no está dentro de una raíz, el informe va a la primera raíz. `.forma-reports/` y la carpeta del documento no pueden ser enlaces simbólicos (se informa en `report_note`). La poda solo borra archivos con el nombre exacto de un informe (`r<rev>-<tool>-<AAAAMMDDTHHMMSSZ>-<n>.json`). Al crear `.forma-reports/` se escribe un `.gitignore` con `*`.
12. **`approval_stale` también avisa de un glifo `approved` sin hash de diseño.**
13. **La etiqueta de historial de `image_trace` cuenta todos los glifos trazados** (`traced_total`), no la lista recortada del resumen.

## Revisión independiente del bloque A (2026-10-18)

Veredicto: cambios solicitados. Hubo 0 hallazgos críticos, 4 importantes y 10 menores.
- Los 4 importantes están corregidos, con su test: decisiones 2, 9, 10 y 11.
- De los menores, se corrigieron 12, 13, la sincronización de los respaldos y el separador final.
- Quedan registrados, sin cambio:
  - Un compuesto aprobado cuyo glifo base cambia no se devuelve a revisión automáticamente: `validate` lo detecta como `approval_stale`.
  - Una base rechazada sigue dibujándose dentro de los compuestos que la usan: `validate` da `glyph.rejected_used_by_component` (error).
  - En `summary`, el `total` de `validate` cuenta los tipos listados (errores y avisos).
