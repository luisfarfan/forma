# Decisiones tomadas durante la ejecución del Hito 2

> Registro de las decisiones ("rulings") que tomó el controlador al ejecutar el plan `docs/superpowers/plans/2026-10-17-hito2-imagen-a-fuente.md` con subagentes, en el orden en que se tomaron. Cada una indica qué se decidió y qué cuesta si resulta equivocada. Las decisiones arquitectónicas mayores están además en `docs/adr/` (0008 y 0009), y las correcciones al diseño, en la sección "Enmiendas tras la implementación" de la especificación.

## Proceso
1. **R1. Rama `hito2` en el mismo checkout, sin worktree.** Es un único esfuerzo y `main` queda intacta, como en el Hito 1. Costo si es incorrecta: ninguno.
2. **R2. Tarea 2: el efecto dominó del kerning se prueba con funciones puras** (`diff`/`changed_entities`, journal `record`/`apply_records`, nombre en el detalle del conflicto). Las pruebas de revertir, restaurar y deshacer kerning pasan a la Tarea 5, donde ya existen los comandos. Costo si es incorrecta: un error del efecto dominó se descubre una tarea más tarde.
3. **R3. Tarea 5: la prueba masiva de 21 pares vive como prueba unitaria** en `forma-engine/src/tools/steps.rs` (`#[cfg(test)]`), no en `tests/approvals.rs`, porque `Steps` es privado del crate. Costo si es incorrecta: ninguno.
4. **R4. Cada tarea que cambia el schema de una herramienta regenera el snapshot de `tools/list` y `docs/api/tools.md`** y declara "aditivo" o "incompatible" en el cuerpo del commit. Se acepta el crecimiento de schemas de herramientas existentes por `Layer.components`, `Font.kerning`, los campos de `FontDiff` y el patrón de `EntityKey`. Costo si es incorrecta: cambios de contrato que el responsable debe releer.
5. **R5. Modelos.** Implementadores opus para las Tareas 2, 4, 5, 10, 12 y 13 (entre crates o algorítmicas) y sonnet para el resto; revisores sonnet, y opus para las Tareas 2, 5, 10, 12 y 13; revisión final con opus. Costo si es incorrecta: solo tiempo y dinero.
6. **R6. La Tarea 22 necesita la ruta de la imagen del responsable.** Si no se entrega al llegar a ella, se informa como pendiente y no impide cerrar el hito. Costo si es incorrecta: la demo local con el alfabeto de la hermana ocurre después de fusionar.
7. **Tarea 1: la revisión I1 (cifras [V] del ADR 0009 sin fuente) no es un defecto.** Las cifras las dio el controlador en el encargo, verificadas con `gh api` y `cargo tree` el 2026-10-17; el implementador solo corrige la redacción de su informe. Costo si es incorrecta: ninguno.
8. **Tarea 2: los menores m1–m4 pasan a la Tarea 5**, que introduce los comandos de kerning que los multiplicarían: poda canónica de mapas vacíos, `match` exhaustivos sobre `EntityKey`, grupos y pares compartidos con `Arc` (`make_mut`) y errores con contexto. Costo si es incorrecta: la Tarea 5 crece un poco.
9. **Tarea 4b: se añade una tarea del controlador** (nesting robusto en `direction.rs`, corte del trazo en cúspides, documentación honesta de `PathOps` y `ARTEFACT_SIZE`, aserción de cobertura en el test de propiedades del trazo). La Tarea 4 encontró un error del Hito 1: `normalize_direction`, `contour_depths` y `check_paths`/`fix_paths` (`WrongDirection`) decidían la contención con el primer nodo de cada contorno, que falla cuando los contornos se tocan en ese nodo; la Tarea 5 (`remove_overlap`) y la Tarea 10 (limpieza del trazado, criterio 2 de `geometry_check`) dependen de ello. Costo si es incorrecta: una tarea pequeña de más.
10. **Tarea 4b: la regla de cortar el trazo cada 10° del brief era incorrecta** (las curvas cerradas pasaban de 18 a 49 nodos). Se corta solo en cúspides reales (giro > 90° o velocidad casi nula). Costo si es incorrecta: un pliegue agudo de menos de 90° recibe un mordisco menor.
11. **Tarea 4b: la sonda de contención sigue de forma perezosa más allá de las 32 aristas más largas** antes de recurrir al primer nodo (la intención del brief era "cada segmento"). Costo si es incorrecta: peor caso más lento con duplicados ambiguos.
12. **Tarea 7: el render lee el glifo almacenado en la fuente por id, no el `Glyph` que se le pasa.** Es un cambio de comportamiento inofensivo: se comprobó que todos los llamadores del motor (`preview.rs:271,284,353`) pasan glifos de la misma fuente del documento. Costo si es incorrecta: ninguno.

## Trazado
13. **Tarea 9: construcción de candidatos.** Se descarta el micro-ruido (< 0,5 % de la mediana); las filas se forman con las piezas grandes (≥ 2 %); las piezas pequeñas se asignan a la banda de una fila (±35 % de su altura) o se descartan; solo se fusiona dentro de una fila con solapamiento horizontal ≥ 50 %; se descarta lo que quede por debajo del 2 % de la mediana; después se ordena. No hay regla de hueco vertical, porque así `: ; = i ¡ ?` quedan enteros y la fusión no se encadena entre filas. Costo si es incorrecta: el polvo dentro de la fila y el rango x de un glifo puede agrandar un poco su caja.
14. **Tarea 9: las imágenes RGBA se componen sobre blanco antes de analizarlas**, porque los escaneos en PNG transparente son comunes. Costo si es incorrecta: ninguno.
15. **Tarea 11: el criterio 1 (IoU ≥ 0,97 contra el contorno real) es vinculante y no se relaja.** El error de 0,25 px de la imagen original (1 px sobremuestreado) sugería un sesgo sistemático, así que se abrió la Tarea 11b de depuración sistemática antes de revisar la Tarea 11. Costo si es incorrecta: una tarea más si era solo ruido de resolución.
16. **Tarea 11b: se aceptan las dos desviaciones de la especificación §6.1**, con causas raíz medidas: la máscara suave vale la cobertura sRGB (no un cociente Lab) y visioncortex solo recorre los bordes (`PathSimplifyMode::None`), con las curvas del `fit` propio. Se enmienda la especificación y se anota en el ADR 0009. Resultado: IoU mínimo de contorno 0,9853 y de máscara 0,9873. Costo si es incorrecta: la especificación va por detrás hasta esta documentación.

## Espaciado y kerning
17. **Tarea 12: la profundidad del 15 % del UPM de la especificación §6.2 era incorrecta** (con ella `O` quedaba en −5/−5 y `A` en −55/−55). Se recalibró con criterios de resultado: margen de `O` en [0,5; 0,9] × `H`, `A` ≥ 0 y ≤ 0,6 × `H`, simetría, idempotencia y `tightness` monótono, con recorte en 0. Constantes finales: profundidad 0,4 × ancho de trazo de `H`, `edge_weight` 0,25 (peso `0,25 + 0,75 · sin(π·t)`). Costo si es incorrecta: el espaciado se ve distinto del texto literal de la especificación original.
18. **Tarea 15: semántica de `kerning_auto` con `replace`.** `replace = true` borra los pares del master elegido y todo grupo cuyos miembros se crucen con el conjunto de glifos kerneado, salvo que los pares de otro master lo referencien. Si un grupo planeado choca de nombre con uno que se conserva, es `INVALID_ARGUMENT` y nombra el grupo. El caso del Hito 2 es la fuente de un master. Costo si es incorrecta: los flujos de kerning con varios masters se revisan en el Hito 5.
19. **Tarea 15: se acepta el rendimiento de `kerning_auto` en el Hito 2.** 89 glifos tardan 7,4 s en debug (`validate_invariants` por comando × 3246; `Steps::of` 3,4 s + ejecutar 3,45 s); el producto corre en release. Un comando de kerning en bloque y una sola validación por transacción se difieren al Hito 4 (fuentes OFL con miles de pares). Costo si es incorrecta: el kerning de fuentes grandes es lento hasta entonces.
20. **Tarea 15: `kerning_auto` con `replace` y un subconjunto `glyphs` solo borra los pares que involucran a esos glifos o a los grupos a los que pertenecen**, no todos los pares del master. Es estrictamente menos destructivo. Costo si es incorrecta: ninguno.

## Herramientas
21. **Tarea 17: `glyph_get` expone los componentes de cada capa** (nombre de la base, id y transformación), porque un agente necesita inspeccionar los compuestos. Es un cambio aditivo del schema de salida. Costo si es incorrecta: ninguno.

## Cambios aditivos del contrato `tools/list`

El snapshot de `tools/list` cambió varias veces durante el hito, siempre de forma **aditiva** (ninguna de las 35 herramientas del Hito 1 perdió un campo ni cambió de tipo):
- `Layer.components` (capas de `glyph_get` y de las herramientas que devuelven capas) y `Font.kerning`, por el formato v2.
- Banderas nuevas en `FontDiff`/`GlyphDiff` (componentes y kerning cambiados) para `document_diff`.
- El patrón de texto de `EntityKey`, que ahora admite `kerning_group:<uuid>` y `kerning_pair:<master>:<ref>:<ref>`.
- `glyph_get`: la lista de componentes de cada capa (decisión 21).
- 15 herramientas nuevas (35 → 50) y tres conjuntos nuevos (`components`, `kerning`, `trace`); `forma_capabilities` gana los límites `max_image_bytes`, `max_image_pixels`, `max_candidates`, `max_assignments` y `max_kerning_check_pairs`.
- Redacción de las descripciones de `edit_batch` (ahora habla de entidades, no de "más de 20 glifos"), `history_revert` y `kerning_auto`. Son cambios de texto, no de schema; el contenido se acortó para respetar el límite de 1000 caracteres, conservando "all apply or none does" y "each on the previous result" en `edit_batch`.
- Observación sobre el comportamiento: un comando que no toca ninguna entidad (nada cambió de verdad) devuelve `tx: null` y no avanza la `rev`, para todas las herramientas mutantes (Tarea 14).

## Diferido

Lista de los menores que siguen abiertos al cerrar el hito (los corregidos en tareas posteriores no aparecen). Ninguno implica pérdida de datos.

**Geometría y booleanas**
- `linesweeper` pierde una región en ~1 de cada 200 000 trazos aleatorios y deja ~0,3 % de cruces residuales con entrada muy autointersecante (documentado). Curvas con radio menor de ~11,5 unidades dan cadenas de esquinas redondas.
- Coste de la sonda de contención en el peor caso con contornos duplicados: O(E²) perezoso. Con contornos que se cruzan sin unir, la contención (y por tanto la dirección) queda indefinida; está documentado en `geometry_fix` (ejecutar antes `remove_overlap`).
- Pruebas de propiedades con semilla fijada pueden diferir en el último decimal entre sistemas (la CI lo dirá); las de `forma-edit` y `forma-store` no tienen semilla (no dependen de linesweeper). Quedan fallos raros de desplazamiento a alcance completo (28 por muestreo).
- Faltan pruebas del respaldo del primer nodo y de la invariancia ante inversión directa.

**Modelo, edición y persistencia**
- `Proposal::summary` (`plans.rs:317`) usa `matches!` sobre `EntityKey`.
- No hay prueba de propiedades del inverso de kerning por `revert`.
- `SetComponents` con una lista idéntica sigue tocando el glifo; `Kerning::is_empty` comprueba valores, no mapas; `older_layer` sobreaproxima (igual que `SetOutline`).
- `revert` de una transacción que creó un glifo no se propaga en cascada (falla limpiamente; documentado).
- El mensaje de desajuste manifiesto/`user_version` es confuso y no tiene prueba; la prueba de guardado con valores no finitos solo comprueba `is_err()`; faltan pruebas directas de despacho para la versión 0 o basura en `migration.rs`.

**Render y máscaras**
- Las pruebas de máscara usan un cuadrado simétrico en diagonal (no detectan transposición, desplazamiento ni cobertura parcial); `InvalidSize` informa el lado máximo, no el número de píxeles; el contorno se llama "mask" en los errores; la discrepancia de dimensiones del IoU se mapea a `Geometry`; la longitud de datos de un `Mask` construido a mano no se comprueba.
- Una máscara de 50 megapíxeles reserva ~200 MB. `MAX_CHORDS` (65 536 por curva) del aplanado expone memoria.
- No hay prueba unitaria directa que fije `Mix::soft`. El PNG del espécimen se verificó byte a byte solo en macOS; en otros sistemas se recurre a comparar píxeles.

**Trazado**
- Un `i` suelto sin letras más altas forma su propia fila (por la decisión 13); la mediana de áreas incluye el micro-ruido, de modo que JPEG ruidosos podrían bajar los umbrales (vigilar con la imagen real).
- Límites conocidos: ápices ≤ 16° caen 1–2 px fuera; trazos finos (5–7,5 px con mayúsculas de 110 px) dan IoU 0,962–0,968; la Λ en inglete queda con poco margen (0,973); curvas muy planas (flecha ≤ min(0,5 px; 1 % de la cuerda)) pueden quedar como líneas; el IoU del contorno de `O` tiene un margen frágil.
- Un candidato defectuoso aborta todo el lote de `image_trace` (ahora el error nombra el candidato y el glifo); el mensaje de "demasiado grande" de `trace_compare` es mejorable.
- `image_inspect` no activa la nota de truncamiento (los campos lo indican); la atribución del reintento solo se probó con un trazador falso; la prueba de symlinks solo corre en Unix.
- `fit_cubics` hace siempre cuatro pasadas de Newton: los resultados de `stroke` y `simplify_path` del Hito 1 cambian ligeramente. Las métricas de fila ahora son fraccionarias (~1 % de cambio de escala).

**Espaciado y kerning**
- Rasgos más finos que el espaciado entre líneas quedan sin detectar por las colisiones (límite documentado; ~4 unidades).
- Faltan pruebas con componentes mezclados con contornos, descendentes, kerning de grupo en colisiones, un segundo grosor de fuente y pares AT/LT/To y el gancho de la `f`.
- `Sidebearings` no es `#[non_exhaustive]`; el `profile()` público conserva la regla de 8 líneas aunque `spacing_auto` no la use; la bisección decorativa.
- El valor de clase llega a miembros que no comparten líneas con el otro lado (documentado). `spacing_auto` incluye `.notdef` por defecto (intencionado). La guardia del kerning puede abrir pares ya más cerrados que 2 % del UPM (+ un kern pequeño).
- `kerning_auto` es lento en debug con muchos glifos (decisión 19). `spacing_auto`, `kerning_auto` e `image_trace` no se pueden usar en `edit_batch`.

**Exportación y compilación**
- `write_ufo` no informa de lo que omite (grupos vacíos filtrados, pares sobre grupos descartados), lo que contradice la regla 8; `check_gpos` usa `masters.first()` en vez de `default_master()`; `KernSide` tiene un respaldo `_ => SECOND`; la prueba de descomposición binaria es débil; hay una línea de documentación sobrante.

**Pruebas de aceptación y CI**
- `scripts/acceptance.ps1`, la ruta de Chrome en Linux y la rama de junction de Windows no se ejecutaron en local (la CI lo dirá). Chrome: `--no-sandbox` incondicional, sin sonda en Windows y respaldo de `.deb` sin fijar ni probar.
- La muestra de pares con kern 0 son los primeros 50 pares ordenados (una selección repartida sería mejor). Un glifo sin codificar en el kerning da un `KeyError` en vez de un mensaje claro. No se captura el número de WARN de fontspector (WARN conocidos: sin STAT, `fsSelection` WWS y `xAvgCharWidth` del espécimen 574 frente a 561).
- Los valores de kern de `expected.json` no están redondeados; el entrecomillado de `mklink` tiene un cabo suelto.
- El fuzzing del decodificador puede entrar en una ruta lenta con PNG de 50 MP muy comprimibles (el límite de 10 s la expone).
- El selector de cobertura de la puerta tiene un recorte inofensivo.
