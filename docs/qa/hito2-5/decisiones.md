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

## Bloque C — Trazado fiel (Tasks 11–13)

14. **Causa raíz de los picos en las serifas** (detectada con el corpus M): la restauración de esquinas de `cleanup` extendía las tangentes de las puntas finas y afiladas hasta encontrarlas lejos de la tinta. Por ejemplo, la `I` de EB Garamond a 200 px se trazaba 400 unidades de ancho en lugar de 326. *Corrección:* una esquina solo se mueve hasta donde se encuentran sus bordes si ese punto queda a ≤ `corner_limit_px` (2 px en el perfil fiel) del borde trazado. La imagen es la evidencia.
15. **Segunda causa de picos: los ajustes por mínimos cuadrados** (`fit_cubics` en `cleanup` y `simplify_path` en `forma-geometry`, del Hito 2). Pasan cerca de sus puntos, pero pueden escaparse entre ellos: en la cuña de la `N` de Fraunces llegaban a y=2596. *Corrección:* toda curva ajustada se verifica contra la geometría que reemplaza (a ≤ 3× la tolerancia y al menos 1 px en `cleanup`; a ≤ 2× el error en `simplify_path`). Si se escapa, el tramo se divide (`cleanup`) o se deja como estaba (`simplify_path`).
16. **`hausdorff_max_px` (2 px) pasa a ser bloqueante** (en §4 de la especificación era solo un aviso). Un pico fino casi no cambia el IoU ni el p95, pero es un defecto visual evidente.
17. **Escalera de reintentos** (enmienda de §5.3). La referencia es siempre la isolínea de la imagen al nivel elegido (0,5 por defecto), porque cambiar la isolínea en un reintento cambiaría la verdad y no el ajuste. Los reintentos solo cambian el ajuste, en este orden:
    1. `subpixel`: suavizado 0,75 px, tolerancia 0,5 px, esquinas a ≤ 2 px.
    2. `subpixel-tight`: 0,4 px / 0,3 px / 1 px.
    3. `subpixel-exact`: sin suavizado, 0,2 px, sin extensión de esquinas.
    4. `edge-walk`: el trazador del Hito 2.

    `isoline` (0,3–0,7) sigue disponible como opción explícita de quien traza. Si ningún intento pasa, se conserva el menos grave (primero sin desvíos grandes, luego la topología, luego los rasgos) y el glifo queda `needs_review` con los motivos.
18. **Bordes duros** (imágenes sin antialiasing, `edge_softness` < 0,25): los escalones de los píxeles son ruido de medio píxel. Se empieza por un perfil que los suaviza (`subpixel-stepped`), y el ancho mínimo de rasgo para estos bordes es 1,5 px. *Límite conocido:* una ranura de 1 px en una imagen de bordes duros no se distingue de un escalón.
19. **"Ranura medio cerrada" solo como contrahueco:** comparar las isolíneas 0,5 y 0,65 marca falsos positivos en todas las esquinas agudas antialiasadas (305 glifos de M). Solo marca `needs_review` un **contrahueco** que aparece en el nivel estricto y no en el 0,5 (evidencia topológica: `inline_outline` en O). Las demás diferencias quedan en `faint`, como dato informativo.
20. **Orientación de las isolíneas por anidamiento** (profundidad par: exterior; impar: contrahueco), no por muestreo puntual. El muestreo se equivocaba cuando la isolínea bilineal se curva dentro de una celda: en la `R` del specimen, el contrahueco de la referencia aparecía relleno.
21. **Agrupación por layout** (`forma_trace::match_rows`): programación dinámica con los dos mejores resultados por estado. El trazo típico se estima como 4 × la mediana de la transformada de distancia de la tinta. Las restricciones duras y la ambigüedad son las de §5.3.

### Corpus, modo C: antes (Hito 2) y después (bloque C)

| Nivel | Pipelines correctos | IoU media contra la verdad | Peor IoU contra la verdad | Huecos correctos | `needs_review` | Errores topológicos sin detectar |
|---|---|---|---|---|---|---|
| S | 1/1 → 1/1 | 0,994 → 0,995 | 0,978 → 0,984 | 8/8 → 8/8 | 0/8 | 0 |
| L | 8/11 → 9/11 | 0,998 → 0,998 | 0,986 → 0,982 | 30/31 → 35/35 | 0/35 | 0 |
| O | 5/7 → 6/7 | 0,981 → 0,984 | 0,946 → 0,953 | 29/30 → 35/36 | 1/36 | 0 |
| M (36 casos) | 36/36 | 0,883 → 0,971 | 0,154 → 0,750 | 929/936 → 935/936 | 96/936 | 0 |
| M-hand (12) | 12/12 | 0,957 → 0,980 | 0,424 → 0,897 | 312/312 | 3/312 | 0 |

**Lectura de la tabla:**
- **L:** los dos casos que siguen sin pasar están bien informados. En `touching_pair` las letras se tocan, y no se separan (fuera de alcance). En `uneven_light` el análisis ve una sola forma; es un límite conocido de la paleta con iluminación muy desigual, pendiente.
- **`needs_review` en M:** 85 de los 96 casos son de c60, letras de 60 px donde el IoU está dominado por el borde y las serifas miden un píxel. Se calibra en la Task 18b.
- **Alfabeto privado** (local, sin publicar imágenes):

  | Letra | Hito 2 | Bloque C |
  |---|---|---|
  | E | IoU 0,936, ranura cerrada | IoU 0,994, ranura conservada, sin revisión |
  | H | no se podía asignar como un glifo | candidatos 7+8 agrupados automáticamente, IoU 0,9925, sin revisión |

## Revisión independiente del bloque C (2026-10-18)

Veredicto: cambios solicitados, con 2 hallazgos críticos, 7 importantes y 11 menores. Todos los críticos e importantes se reprodujeron y se corrigieron. Las correcciones se verificaron con los escenarios de reproducción del revisor.

22. **(Crítico) Cuelgue de Hausdorff con un conjunto vacío.** La búsqueda por anillos queda acotada a las celdas ocupadas, recorre solo el perímetro de cada anillo y devuelve el resultado al instante si un lado no tiene borde. Una referencia vacía (la tinta no alcanza la isolínea) o un contorno vacío marcan el glifo `needs_review` con el motivo.
23. **(Crítico) La agrupación podía unir letras vecinas** (H I O: la barra de la H a 7 px de su asta y la I a 4 px). Ahora:
    - **Contraste obligatorio:** el hueco dentro de un grupo debe ser menor que 0,5 × el menor hueco entre los grupos de la fila (las piezas de una letra están más juntas que las letras).
    - **Ambigüedad:** la segunda mejor partición debe costar al menos 2× la mejor (antes 1,15×); si no, la fila queda `ambiguous_grouping`.
    - **Límite de hueco:** se mantiene el de §5.3 (0,8 × trazo, con un mínimo de 1,5 px).

    La H del alfabeto privado (piezas solapadas), `multi_piece_h` y `stencil` se siguen agrupando solas; H I O queda informado.
24. **(Importante) Desviación de ajuste (§4):** `fit_deviation_px` (percentil 99 de la distancia contorno ↔ isolínea) es bloqueante, con 0,5 px en `faithful` y 1,0 px en `clean` y en bordes duros. Se suma media unidad de fuente en píxeles (`grid_px`), porque las coordenadas se redondean a unidades enteras y ningún contorno puede seguir la tinta con más detalle que eso.
25. **(Importante) Glifos grandes:** los rásters de calidad se adaptan (como máximo 4 muestras por píxel y 4 Mmuestras en total). El error de un intento ya no aborta la llamada. Un glifo de 1900 px se traza en 1,6 s.
26. **(Importante) Imágenes de alta resolución:** el suelo de simplificación es ¼ de píxel de origen (entre 0,25 y 1,5 unidades), no 1,5 unidades fijas.
27. **(Importante) Estimación del trazo:** 2 × la mediana de la transformada de distancia sobre la cresta de la tinta, menos 1 px. Antes, 4 × la mediana, sesgada en unos 2 px.
28. **(Importante) `candidates`:** se une la tinta propia de cada candidato. Los otros candidatos que caen dentro de la caja común ya no se cuelan.
29. **(Importante) El IoU de la puerta se mide contra la isolínea al nivel trazado** (rasterizada), no contra la máscara al 0,5. `iou` sigue informando el IoU contra la máscara.
30. **(Importante) Bordes duros:** solo cuentan los píxeles de escalón (con cruce a la vez horizontal y vertical). Los bordes alineados con la cuadrícula son exactos y no hacen que una imagen antialiasada parezca dura.
31. **(Menor)** Los contrahuecos se emparejan con IoU ≥ 0,5 (§4 decía 0,8; con contrahuecos de pocos píxeles, 0,8 rechaza trazados correctos; se recalibra en la Task 18b). La procedencia registra `attempt` e `isoline`.
32. **Control nuevo: autointersección** (`crossings`, bloqueante). Un contorno que se cruza crea un lazo que el relleno no-cero oculta, pero que el editor muestra.
33. **Límite de medición conocido:** el benchmark cuenta huecos en un ráster más fino que el control de calidad. Dos glifos de M a 60 px tienen muescas estrechadas por debajo de ¼ de píxel, que el benchmark ve como contrahueco y el control no. No es un error del dibujo; se revisa en la calibración.

### Corpus, modo C tras las correcciones del bloque C

| Nivel | Pipelines correctos | IoU media contra la verdad | Peor IoU | Huecos | `needs_review` |
|---|---|---|---|---|---|
| S | 1/1 | 0,995 | 0,984 | 8/8 | 0/8 |
| L | 9/11 | 0,998 | 0,983 | 35/35 | 1/35 |
| O | 6/7 | 0,983 | 0,944 | 35/36 | 2/36 |
| M | 36/36 | 0,974 | 0,569 | 931/936 | 25/936 |
| M-hand | 12/12 | 0,984 | 0,906 | 312/312 | 0/312 |
