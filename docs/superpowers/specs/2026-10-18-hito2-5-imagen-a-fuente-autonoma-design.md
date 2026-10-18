# Hito 2.5 — De una imagen a una fuente con mínima intervención de IA — Diseño

> Estado: **aprobado con ajustes** por el responsable el 2026-10-18 (ver §11). La dirección fue aprobada en la conversación del 2026-10-18 tras la auditoría del pipeline del Hito 2.
> Rama: `hito2-5`, desde `main` @ `26a60f3` (Hito 2 fusionado).
> Prioridad, en este orden:
> 1. **máxima calidad tipográfica y fidelidad al original**;
> 2. **automatización inteligente**;
> 3. **mínimo consumo de tokens**;
> 4. velocidad.
>
> Ninguna meta de costo o de tiempo justifica sacrificar calidad.

## 1. Contexto y línea base medida

El Hito 2 ya convierte la imagen de un alfabeto en una fuente completa. La auditoría del 2026-10-18 midió dos ejecuciones reales con Claude Code (Opus 5.5) y el motor local por CLI.

| | Specimen (8 letras + 0 + tilde) | **Alphabetical** (26 letras + 52 glifos diseñados) |
|---|---|---|
| Duración del agente | 153 s | **986 s** |
| Motor local (mismas etapas, por CLI) | ~0,5 s | **~1,2 s** |
| Turnos | 43 | **173** |
| Tokens de entrada (de ellos, leídos de caché) | 5,0 M | **14,3 M** (14,06 M de caché) |
| Tokens de salida (de ellos, razonamiento) | ~9 k | **101 k** (35,7 k) |
| Costo reportado por el cliente | 1,34 USD | **6,94 USD** |
| Contexto al empezar → al terminar | 41 k → 92 k | 41 k → 262 k |

**Dónde se fue el trabajo en Alphabetical:**
- **Diseñar 52 glifos a mano fue ~70 % de los turnos:**
  - 13 `edit_batch` con 63 000 caracteres de coordenadas escritas por el modelo;
  - 77 `preview_glyph` con imagen, para revisarlos uno a uno;
  - dos rehacer completos.
- **Errores de máscara y topología que pagó la IA (~25 llamadas):**
  - la ranura fina de la E se cerró (IoU 0,936);
  - la H, dibujada en dos piezas separadas, no se pudo asignar como un solo glifo.
- **Respuestas largas releídas en cada turno:**
  - 9 `spacing_auto` (18 k caracteres en total);
  - `geometry_check` (17 k);
  - `kerning_auto` (11,5 k).
- **Catálogo de herramientas:** `tools/list` pesa 315 k caracteres, de los cuales 188 k son *output schemas*. Cuánto pesa en cada turno depende del cliente: Claude Code carga las herramientas de forma diferida, y otros clientes las cargan enteras. **No se supone ningún ahorro: se mide por cliente** (§8).

Son líneas base que se conservan como referencia.

## 2. Objetivo

FORMA recibe un alfabeto dibujado a mano y produce una **fuente profesional, editable y fiel al original**, con la mínima intervención de IA. La IA supervisa, interpreta casos ambiguos y aprueba o corrige excepciones; **no dibuja coordenadas**.

### Fuera de alcance
- Reconocer letras sin layout (OCR) o separar letras que se tocan.
- Generar minúsculas propias: el perfil sigue siendo `unicase-es`.
- Multi-master y fuentes variables.
- La interfaz gráfica (Hito 8).
- Cualquier servicio externo o modelo de IA nuevo para resolver geometría.

## 3. Principios

1. **Fidelidad antes que limpieza.** El vectorizador no suaviza por debajo del nivel de ruido medido de la imagen ni convierte una forma orgánica en geométrica. Toda simplificación tiene un presupuesto de desviación en **píxeles de la imagen de origen**, no en unidades de fuente.
2. **Error de vectorización ≠ decisión artística.** Una irregularidad es **artística** si:
   - es estable al variar la binarización dentro de la tolerancia;
   - es mayor que el nivel de ruido;
   - está sostenida por varios píxeles.

   Se conserva. Es **ruido** si cambia con la binarización o es menor que el nivel de ruido. Ante la duda, se conserva y se informa.
3. **Una contraforma perdida nunca se compensa con un IoU alto.** La topología se valida aparte del IoU (§4) y bloquea.
4. **Todo glifo generado se identifica como tal** (procedencia, receta, puntuación de estilo), es editable y queda **pendiente de aprobación** hasta que alguien lo apruebe.
5. **Mejor pedir ayuda que producir un glifo mediocre.** Si la síntesis no alcanza el umbral de estilo, el glifo no se crea y se informa como `needs_design`.
6. **Arquitectura intacta:**
   - Rust como núcleo y un solo registro de comandos, con paridad CLI/MCP;
   - transacciones con revisiones, aprobaciones e historial;
   - fachadas sin lógica;
   - compatibilidad de documentos con migraciones puras;
   - CI en macOS, Windows y Ubuntu.

## 4. Modelo de calidad (métricas y umbrales)

Todas las métricas se calculan en Rust, se guardan en el informe en archivo (§5.4) y se resumen en la respuesta MCP. Las definiciones son el contrato del benchmark (§8).

**Los umbrales son iniciales y experimentales, no constantes universales.**
- Viven en una estructura versionada `QualityThresholds { version, … }`.
- Cada informe y cada procedencia registran la versión de umbrales con la que se decidió y las razones de cada aceptación o rechazo.
- Se calibran (§8.5) con fuentes geométricas, serif, sans, orgánicas y manuscritas antes de consolidarlos. Si la calibración pide cambiarlos, el cambio se registra con su evidencia. Si un cambio relajara la fidelidad, se consulta al responsable.

**Las puertas son conjuntivas.** Un glifo pasa solo si pasa **todas** las bloqueantes: un IoU alto nunca compensa una topología incorrecta, un rasgo fino perdido o una desviación local grande. **Ante ambigüedad, `needs_review`.**

**Dos familias de evaluación separadas**, que nunca se mezclan en una sola cifra:
- **Fidelidad geométrica:** el parecido con la imagen. Se mide con las métricas de esta tabla.
- **Calidad tipográfica:** espaciado, kerning, coherencia estilística entre glifos y calidad de curvas para la edición (§8.3).

| Métrica | Definición | Umbral por defecto | Bloquea |
|---|---|---|---|
| `iou_mask` | IoU del contorno rasterizado contra la máscara del candidato, en píxeles de origen (ya existe) | ≥ 0,97 | sí |
| `hausdorff_p95_px` | Percentil 95 de la distancia contorno → borde de la máscara, en px de origen | ≤ 0,75 px | sí |
| `hausdorff_max_px` | Distancia máxima | ≤ 2 px (si se supera: aviso con la ubicación) | no |
| `topology` | Mismo número de componentes y de huecos (número de Euler) en la máscara y en el contorno rasterizado, con cada hueco emparejado (IoU de hueco ≥ 0,8) | igualdad | **sí** |
| `thin_features` | Rasgos finos de la máscara (ranuras y trazos con ancho local ≥ 1 px según la transformada de distancia) presentes en el contorno | 100 % | **sí** |
| `curve_quality` | Nodos on-curve por 1000 unidades de perímetro; extremos con nodo; continuidad G1 en nodos `smooth` (≤ 3°); sin segmentos diminutos | nodos ≤ límite por perímetro; 0 violaciones G1; `geometry_check` limpio | sí (G1 y geometría); el límite de nodos solo avisa |
| `fit_deviation_px` | Desviación máxima de la curva ajustada respecto al borde subpíxel | ≤ presupuesto de fidelidad (0,5 px en modo `faithful`) | sí |

**Modo de fidelidad:**
- `fidelity: "faithful"` (por defecto): presupuesto de 0,5 px y conserva irregularidades estables.
- `fidelity: "clean"`: presupuesto de 1 px; suaviza irregularidades menores que el nivel de ruido.

El nivel de ruido se estima por imagen: la dispersión del borde subpíxel en tramos rectos largos, con un piso de 0,25 px.

**No homogeneizar.** El motor no tiene un "estilo correcto" al que acercar los glifos. En `faithful`, las imperfecciones estables forman parte de la identidad de la fuente (como en Alphabetical) y se conservan. El benchmark incluye casos orgánicos y manuscritos cuya irregularidad debe sobrevivir medida (§8.1).

## 5. Diseño

### 5.1 Fase 1 — Contexto y respuestas compactas, medidos

**Medir primero.**
- `tools/bench/agent_metrics.py`:
  - analiza el `stream-json` de Claude Code y el `--json` de Codex;
  - extrae turnos, llamadas por herramienta, bytes de argumentos y respuestas, e imágenes;
  - extrae los tokens de entrada, caché (leída y escrita), salida y razonamiento, el costo reportado y la duración.
- `forma mcp --print-catalog-size` informa el tamaño de `tools/list` por perfil, en caracteres y en tokens estimados (como estimación, no como medida del cliente).

**Respuestas compactas.**
- Nuevo argumento opcional `detail: "summary" | "full"` (por defecto `summary`) en las herramientas que hoy devuelven listas largas: `spacing_auto`, `kerning_auto`, `kerning_get`, `kerning_check`, `geometry_check`, `geometry_fix`, `validate`, `image_trace`, `charset_check`.
- `summary` devuelve totales, **solo las excepciones** (fallos, recortes, colisiones, avisos) y `report_path`.
- El detalle completo se escribe en un JSON canónico (§5.4).
- Es un cambio **aditivo**: el argumento es nuevo y el resumen conserva los campos existentes, recortando las listas largas con `*_total` y `*_truncated`, como ya hacen otras herramientas.

**Revisión visual en una sola imagen.**
- Herramienta nueva `preview_sheet` (lectura, salida imagen):
  - hoja de contacto de N glifos (por nombres o por filtro: `synthesized`, `needs_review`, `traced`, `all`) en una grilla;
  - cada celda lleva el nombre del glifo, un badge de procedencia (trazado, sintetizado, compuesto, manual), el estado de revisión y, opcionalmente, la superposición contra la imagen de origen para los trazados;
  - presupuesto de 1600 × 1600 px y celdas de 96–256 px, que se reducen para que quepa.
- `preview_text` gana `wrap: true`, que corta en varias líneas en lugar de recortar a 2048 px.

**Perfiles de herramientas.**
- `forma mcp --profile image-font` sirve solo el subconjunto que necesita el flujo imagen → fuente: `forma_capabilities`, documentos, `font_from_image`, `preview_sheet`, `glyph_review`, `glyph_get`, edición granular de contornos, historial, `validate` y `export_font`.
- `--toolsets` sigue funcionando igual.

**Output schemas opcionales.**
- `forma mcp --output-schemas none` omite los `outputSchema`, que MCP permite omitir.
- **Por defecto no cambia nada**, para no romper a los clientes.

### 5.2 Fase 2 — `font_from_image`, la operación principal

**Comando del registro** (toolset `trace`; efecto declarado DESTRUCTIVE, porque puede reemplazar glifos; mutante, con `doc` y `base_rev`):

```text
font_from_image {
  doc, base_rev, dry_run?,
  path,                                  # imagen dentro de --allow-dir
  layout?: ["ABCDEFGHI", "JKLMNOPQ", "RSTUVWXYZ"] | "auto",
  extra?: [{ row, index_from_end|index, name, unicodes }],   # p. ej. llaves de una fila decorativa
  profile?: "unicase-es",
  fidelity?: "faithful" | "clean",
  stages?: { synthesize: true, compose: true, space: true, kern: true, export: { ttf?, woff2? } },
  color?, tolerance?, master?,
  detail?: "summary" | "full"
}
```

**Orquestación** (todo en el motor, sin lógica en las fachadas):
1. `analyze` → **asignación por layout** (§5.3) → trazado de todas las asignaciones (§5.3) → controles de calidad (§4).
2. Extracción del estilo y síntesis de lo que falta (§5.5, si `synthesize`).
3. Composición de acentos (si `compose`).
4. `spacing_auto` → `kerning_auto` → `kerning_check`.
5. `charset_check` y `validate`.

**Una transacción.**
- Todos los cambios del documento se aplican en **una sola transacción** etiquetada `font_from_image`, deshacible con un `history_undo`.
- Se pasa por `Steps`, así que valen las revisiones por entidad, los conflictos y la clasificación destructiva o masiva.

**Aprobación.**
- Crear muchos glifos es masivo. En la política `prompted` el comando devuelve **un único plan** con el resumen, los avisos y el diff; se aprueba con un `plan_commit`, que necesita un humano.
- En `auto` se aplica directo. Reemplazar glifos existentes es siempre destructivo.
- **No cambia ninguna regla de seguridad.**

**Exportación.**
- Se ejecuta después del commit, con la misma semántica de archivos que `export_font`: sobrescribir un archivo existente pide plan.
- Si hay glifos `needs_review` o sintetizados `pending`, la exportación se **omite** y se informa, salvo que se pase `export: { allow_pending: true }`.

**Respuesta compacta:**
- por etapa: estado, conteos y tiempos;
- `attention`: asignaciones ambiguas, glifos que no pasan un control (con el motivo) y síntesis rechazadas (`needs_design`);
- `report_path`.

La revisión visual se hace con **una** llamada a `preview_sheet`.

**Granularidad preservada.** `image_inspect`, `image_trace`, `glyph_stroke`, `glyph_compose`, `spacing_auto`, `kerning_*` y el resto no cambian, salvo los argumentos aditivos de las fases 1 y 3.

**Flujo esperado del agente en el caso normal (hipótesis que se mide en §8):**

```text
document_create → font_from_image (dry_run) → plan_commit → preview_sheet
→ glyph_review / correcciones puntuales → export_font
```

### 5.3 Fase 3 — Segmentación, máscara y topología

**Borde subpíxel.**
- Nuevo extractor de contornos por **marching squares** sobre la cobertura suave (la cobertura sRGB del Hito 2), a la resolución original.
  - Corta en la isolínea 0,5, con interpolación lineal y desambiguación de las celdas silla por el promedio de la celda.
  - Reemplaza el sobremuestreo ×4 bilineal + binarización + recorrido de bordes con visioncortex, que cierra ranuras de 1–2 px.
- visioncortex queda como alternativa (`extractor: "edge-walk"`) para comparar en el benchmark.
- Después sigue el ajuste propio actual (detección de esquinas y Schneider), con el **presupuesto en px de origen** de §4.

**Validación topológica (§4).**
- Por candidato: componentes y huecos de la máscara (conectividad 8 para la tinta y 4 para el fondo).
- Por glifo trazado: lo mismo sobre su rasterización.
- **Rasgos finos:** ranuras (fondo dentro de la caja con ancho local ≥ 1 px) y trazos finos, cada uno emparejado con el resultado.
- **Si falla:**
  - el motor reintenta, en este orden: isolínea 0,45 y 0,55, `fidelity: "faithful"` con presupuesto 0,25 px, y extractor alternativo;
  - si sigue fallando, el glifo queda `needs_review` con su recorte y el motivo ("falta 1 hueco", "ranura de 1,4 px cerrada en (x, y)").

**Glifos de varias piezas.**
- `image_trace` gana `candidates: [i, j, …]` por asignación; es exclusivo con `candidate`, y el cambio es aditivo.
- **Agrupación automática, solo con layout y solo dentro de una fila.** Si una fila tiene más piezas que caracteres esperados, se busca por programación dinámica la partición de la secuencia ordenada de piezas en exactamente *k* grupos contiguos que minimiza un costo:
  - separación horizontal entre piezas, relativa al trazo medido;
  - desviación del ancho del grupo respecto a la mediana de la fila;
  - penalización si el grupo supera 1,6 × la mediana de ancho.
- **Restricciones duras:** solo se unen piezas vecinas, con separación < 0,8 × trazo y solapamiento vertical ≥ 50 % de la menor; nunca entre filas.
- **Si no hay una solución clara** (la segunda mejor partición cuesta menos de 1,15 × la mejor), el caso es **ambiguo** y va a `attention` con la imagen anotada de esa fila.
- **Sin layout no se une nada que no se solape;** los pares sospechosos se informan.
- Las letras que se tocan (menos piezas que caracteres) se informan, pero no se separan (fuera de alcance).

### 5.4 Informes en archivo y procedencia

**Informe.** Es diagnóstico, reproducible y prescindible: borrar `.forma-reports/` nunca pierde información del documento. La procedencia, la revisión y las métricas esenciales de cada glifo viven **dentro** del documento.
- `font_from_image` y las herramientas con `detail` escriben un JSON canónico en `<carpeta del documento>/.forma-reports/<documento>/r<rev>-<herramienta>-<run>.json`, dentro de las raíces permitidas.
  - `<documento>` es el id estable del documento.
  - `<rev>` es la revisión sobre la que se calculó.
  - `<run>` es un id de ejecución único (tiempo UTC + contador).
  - Se crea con apertura exclusiva: **nunca sobrescribe** un informe anterior.
- La cabecera del informe lleva el documento, la revisión, la herramienta, el hash de los argumentos, la versión del motor y la versión de umbrales, para poder reproducirlo.
- Contiene el detalle útil: métricas por glifo, tiempos por etapa, decisiones de asignación, recetas de síntesis y puntuaciones. **No** lleva píxeles ni contornos completos, que ya están en el documento o se recalculan.
- **Sin crecimiento ilimitado:**
  - se guardan los 20 informes más recientes por documento y herramienta, con un tope de 20 MB por documento;
  - al escribir uno nuevo se podan los más antiguos;
  - un informe > 2 MB se recorta con un aviso.
- La respuesta MCP solo lleva la ruta y el resumen.
- Se registra en el log de auditoría.

**Procedencia en el modelo (formato v3).**
- `Glyph` gana `#[serde(default)] provenance: Option<Provenance>`.
- `Provenance` tiene:
  - `source`: `traced`, `synthesized`, `composed` o `manual`;
  - `review`: `pending`, `approved` o `rejected`;
  - `detail`: las métricas de trazado (candidato o candidatos, `iou_mask`, `hausdorff_p95_px`, `topology_ok`) o la receta de síntesis (id, glifos fuente, puntuación de estilo).
- `Provenance` también registra `edited: bool`: se activa cuando un glifo trazado, sintetizado o compuesto recibe una edición geométrica posterior. Así se distinguen los cuatro casos pedidos: trazado, sintetizado, compuesto y editado manualmente.
- `FORMAT_VERSION` pasa a 3 con una migración v1/v2 → v3 **pura y determinista**: no añade procedencia y no pierde datos (test de ida y vuelta campo por campo con fixtures de v1 y v2).
- **Respaldo:** la primera vez que se guarda en v3 un documento leído en v1 o v2, se escribe antes una copia intacta (`<archivo>.v<n>.bak`, o la carpeta `.formapackage` equivalente), si no existe ya. Esa copia es la vía para revertir la migración.
- **Las aprobaciones solo se invalidan por cambios de diseño:**
  - **Invalidan** la aprobación (`approved` → `pending`, `edited = true`), conservando el origen: los cambios **geométricos**, como contornos, nodos, componentes y sus transformaciones, o quitar solapamientos.
  - **No invalidan** la aprobación: los cambios de **métricas** (avance, márgenes, kerning) ni los de **metadatos** (nombre, unicodes, exportación, procedencia).
  - Al aprobar se guarda el `geometry_hash` del diseño aprobado. Si el documento se modifica por fuera, por ejemplo con git en un `.formapackage`, `validate` detecta que el hash no coincide y avisa.
- Historial, transacciones y concurrencia no cambian: la procedencia es parte del glifo, así que se deshace, se versiona por revisión y participa en `REVISION_CONFLICT`.
- **Herramienta nueva `glyph_review { glyphs, decision: approve | reject }`** (mutante, deshacible). Rechazar no borra el glifo: lo marca y lo excluye de la exportación.
- **Chequeo nuevo de `validate`:** `glyph.pending_review` (aviso) por cada glifo sintetizado o trazado `needs_review` sin aprobar.

### 5.5 Fase 4 — `glyph_synthesize`, síntesis estilística

**Objetivo:** crear los caracteres que faltan (cifras, puntuación, símbolos, marcas) **a partir de la morfología real** de los glifos trazados, no con plantillas genéricas a las que solo se les pasa un grosor.

**Paso 1: kit de estilo** (`style_extract`, interno y también como herramienta de lectura). Se calcula sobre los glifos trazados, aprobados o no:
- **Esqueleto y perfil de grosor:** el eje medio de cada glifo (sobre la transformada de distancia ya existente) con el grosor muestreado a lo largo del trazo. De ahí salen la distribución de grosores, el **contraste** (grueso y fino según la dirección del trazo) y el eje de modulación.
- **Catálogo de terminales:** los extremos reales del contorno, recortados en el cuello (donde el grosor vuelve a la mediana) y normalizados, con forma, tamaño relativo y orientación. Las bolas de esta fuente se copian, no se aproximan con círculos.
- **Uniones:** el estrechamiento en los nodos de grado ≥ 3 (`join_min` ya existe) y el perfil de la unión.
- **Curvatura:**
  - el exponente de superelipse de los ojos y las panzas (O, C, D, Q, G) y los radios de redondeo de las esquinas;
  - la **firma de irregularidad**: espectro de la desviación del contorno respecto a su ajuste suave, para saber si la fuente es orgánica y cuánto.
- **Proporciones:** ancho relativo de redondas, rectas y diagonales; altura de mayúsculas y *overshoot*; distribución de márgenes; gris (área de tinta por em).

**Paso 2: recetas por carácter** (datos versionados, originales).
- Archivo `core/crates/forma-synth/recipes/unicase-es.json`. Cada carácter lleva una lista ordenada de estrategias:
  1. **Derivar de piezas existentes**, cuando existen. Ejemplos:
     - `zero` desde el esqueleto de `O`, reescalado al ancho de las cifras y re-trazado con el perfil de `O`;
     - `one` desde `I` con un terminal del catálogo;
     - `period` desde una bola terminal real;
     - `comma` desde una bola con la cola de `J` o `Q`;
     - `hyphen` desde el travesaño de `H`;
     - `dollar` desde `S` + un trazo vertical.

     Operaciones disponibles: extraer esqueleto, transformar el esqueleto (afín o con deformación suave), re-trazar con un perfil muestreado de la fuente, cortar una pieza por el cuello, pegar un terminal del catálogo, unir.
  2. **Esqueleto paramétrico con estilo transferido**, cuando no hay piezas que derivar. El esqueleto del carácter tiene proporciones ajustadas a las medidas de la fuente, y se viste con el **perfil de grosor muestreado**, los **terminales del catálogo**, las **uniones medidas** y la **firma de irregularidad** (opcional, solo si la fuente es orgánica).
- Los esqueletos paramétricos son trabajo original de FORMA.

**Paso 3: puntuación de estilo y decisión.**
- Cada glifo generado se compara con la distribución de los glifos trazados: grosor (z-score), terminales (distancia de Hausdorff normalizada al catálogo), curvatura, gris, firma de irregularidad y relación de proporciones.
- **Por debajo del umbral** (inicial y experimental: 0,75 sobre 1), o con **cualquier componente < 0,5** (un buen promedio no oculta un terminal o un grosor ajeno a la fuente), el glifo **no se crea**: queda `needs_design` en `attention`, con el motivo y un boceto en el informe.
- **Ningún glifo sintetizado se aprueba automáticamente**, en ninguna política. Aprobarlo exige un `glyph_review` explícito.
- **Por encima**, se crea con `provenance.source = synthesized`, `review = pending`, la receta y la puntuación.

**Herramienta.**
- `glyph_synthesize { doc, base_rev, glyphs?: [...] | profile, min_style_score?, dry_run? }`, mutante.
- La usa `font_from_image` y también un agente o una persona por separado.

### 5.6 Crates y comandos

**Crate nuevo `forma-synth`** (kit de estilo, recetas, síntesis y puntuación). Depende de `forma-geometry`, `forma-model`, `forma-render` (para medir el gris) y `forma-trace` (esqueleto y transformada de distancia).

**`forma-trace`.** Gana el extractor por marching squares, la topología y los rasgos finos, la estimación de ruido, la agrupación por layout y la fidelidad en px.

**`forma-engine`.** Herramientas nuevas:
- `font_from_image`
- `preview_sheet`
- `glyph_synthesize`
- `glyph_review`
- `style_extract` (lectura)

Cambios aditivos: `detail` en las herramientas de lista, `candidates` en `image_trace`, `wrap` en `preview_text`, `--profile` y `--output-schemas` en `forma mcp`.

**Otros crates:**
- `forma-model`: `Provenance`.
- `forma-store`: formato v3 y migración.
- `forma-validate`: `glyph.pending_review`.
- `forma-render`: hojas de contacto con badges y superposición.

**ADR 0010:** operación de alto nivel orquestada en el motor, modelo de procedencia y revisión, síntesis por recetas y formato v3.

## 6. Seguridad y límites
- Mismos límites de imagen que el Hito 2.
- `font_from_image` respeta `MAX_BATCH_OPS` y `MAX_NODES` por transacción; si se supera, `LIMIT_EXCEEDED` con el detalle.
- `preview_sheet` limita el número de celdas (≤ 120) y el tamaño en píxeles.
- Los informes se escriben solo dentro de las raíces permitidas.
- El contenido de la imagen y los nombres siguen siendo datos, nunca instrucciones.

## 7. Compatibilidad
- Documentos v1 y v2 se leen y migran a v3 sin pérdida.
- El snapshot de `tools/list` cambia de forma **aditiva**: herramientas nuevas, argumentos opcionales nuevos y campos nuevos en salidas.
- Las 50 herramientas existentes mantienen su comportamiento por defecto, salvo que las respuestas de lista pasan a `detail: "summary"` por defecto. Eso conserva todos los campos existentes, pero recorta las listas largas.
  - Riesgo: un cliente que contaba con listas completas debe pedir `detail: "full"`.
  - Se clasifica como cambio de comportamiento compatible a nivel de schema y se documenta en el PR y en `decisiones.md`.

## 8. Benchmark y criterios de aceptación

### 8.1 Corpus

Solo imágenes propias, de dominio público u OFL; las privadas, en local.

| Nivel | Contenido | Verdad conocida |
|---|---|---|
| **S** | El specimen propio del Hito 2 (8 letras) | Contornos verdaderos |
| **M** | 3 alfabetos de 26 letras renderizados por código desde fuentes **OFL** de estilos distintos (una grotesca, una serif, una display orgánica), con licencia junto al fixture; degradaciones deterministas: JPEG, ruido, rotación de 1–2°, escala de altura de mayúsculas de 60, 110 y 200 px | Contornos, espaciado y kerning de la fuente original |
| **M-hand** | El mismo tratamiento con una fuente OFL manuscrita (propuesta: **Patrick Hand**) | Igual que M |
| **L** | Casos difíciles generados por código: piezas múltiples, ranuras de 1–3 px, contraformas pequeñas, iluminación irregular, fila decorativa, letras que se tocan (debe informarse, no separarse) | Contornos y topología |
| **O** (orgánico y complejo) | Alfabetos generados por código: <ul><li>trazo manuscrito con presión variable;</li><li>pincel con borde rugoso;</li><li>tinta corrida;</li><li>terminales de bola irregulares al estilo de Alphabetical (propio, no copiado);</li><li>stencil de muchas piezas;</li><li>letras con contorno y relleno (huecos dentro de huecos);</li><li>geometría compleja con muchas contraformas y ranuras</li></ul> | Contornos, topología y **amplitud de irregularidad conocida**, que debe conservarse |
| **Privado** | El alfabeto de la hermana del responsable (`~/fonts/hermana/`) | Sin verdad: IoU contra la máscara, topología y revisión humana. Nunca entra al repositorio ni a los resultados publicados. |

### 8.2 Modos comparados

- **A:** pipeline actual de `main` (Hito 2, herramientas granulares), con agente libre y la misma instrucción.
- **B:** pipeline optimizado (`font_from_image`, hojas de contacto y síntesis), con agente.
- **C:** solo CLI y motor, sin IA (con el layout dado). Da el piso de tiempo y el techo de automatización.
- **Clientes:** Claude Code y Codex.
- **Presupuesto controlado de las pruebas con agentes:**
  - **Línea base A**, con las transcripciones existentes:
    - Alphabetical y specimen con Claude Code;
    - transcripciones del Hito 1 con Codex, solo para el costo del catálogo.
  - No se repite el flujo antiguo. Solo se añade una corrida A con Codex sobre S, la más barata, porque no existe una transcripción de Codex de imagen a fuente.
  - **Modo B:**
    - Claude Code: 3 repeticiones sobre S y 1 sobre un caso M;
    - Codex: 1 sobre S;
    - el alfabeto privado: 1 corrida local.
  - Tope total de **US$ 15** para todas las corridas con agente; cada una tiene tope de turnos.
- **Pruebas deterministas (modo C)** frecuentes y baratas: S, L y O en cada CI; M y M-hand cada noche.

### 8.3 Métricas
- **Tiempo:** total, del motor por etapa (de los tiempos del informe) y del agente (total − motor).
- **Agente:** turnos, llamadas MCP por herramienta, bytes de argumentos y respuestas, imágenes recibidas.
- **Tokens, por cliente:** entrada, caché leída y escrita, salida y razonamiento.
- **Costo:** el que reporta el cliente; si no lo reporta, se calcula con una tabla de precios versionada en el script.
- **Fidelidad geométrica** (separada de la calidad tipográfica):
  - `iou_mask`; IoU contra el contorno verdadero (M, L y O);
  - `hausdorff_p95_px`;
  - topología (% de glifos correctos y casos no detectados);
  - rasgos finos conservados;
  - **irregularidad conservada** (O: amplitud medida / amplitud conocida).
- **Curvas:** nodos por perímetro, violaciones G1 y `geometry_check`.
- **Calidad tipográfica — espaciado y kerning (M):**
  - error medio absoluto de los márgenes frente a la fuente original, en % del UPM;
  - correlación y error medio absoluto del kerning en los pares comunes;
  - colisiones.
- **Calidad tipográfica — síntesis:** puntuación de estilo, % creados contra `needs_design`, glifos sintetizados rechazados o editados en la revisión humana.
- **Validez:** `validate`, OTS (TTF y WOFF2), fontspector (FAIL y WARN), HarfBuzz y cobertura.
- **Intervención humana:** número de aprobaciones, correcciones manuales y llamadas de edición después del pipeline.

### 8.4 Criterios de aceptación del hito
1. **Calidad sin regresión (bloqueante):**
   - en S, M y L, el modo B iguala o supera al A en IoU (máscara y verdad), Hausdorff, topología y validez;
   - **0 contraformas perdidas no detectadas** en L y O;
   - 100 % de los rasgos finos de ≥ 1 px conservados o informados;
   - en O, en modo `faithful`, la irregularidad conocida se conserva ≥ 80 % (el motor no homogeneiza).
2. **La E y la H del alfabeto privado se resuelven sin intervención:**
   - la E con la ranura conservada (topología correcta, IoU ≥ 0,97);
   - la H agrupada automáticamente con el layout;
   - **ninguna unión accidental** de letras vecinas en S, M y L.
3. **Síntesis:**
   - todo glifo sintetizado está marcado y pendiente de revisión;
   - en M, la puntuación de estilo de lo aceptado es ≥ 0,75;
   - los glifos por debajo del umbral salen como `needs_design`, nunca creados en silencio.
4. **Automatización:**
   - el modo C produce una fuente válida (S y M) con el layout dado y sin IA;
   - el modo B no necesita coordenadas escritas por el agente en el caso normal.
5. **Consumo medido y publicado** (sin cifras prometidas): tabla A contra B por cliente con todas las métricas de §8.3, incluida la comparación con la línea base de Alphabetical (986 s, 173 turnos, 6,94 USD).
   - **Las metas de "< 2 min" y "< 0,50 USD" son hipótesis que el benchmark confirma o refuta.**
6. **Disciplina:**
   - cada herramienta nueva es un comando del registro con tests y paridad MCP/CLI;
   - snapshot aditivo;
   - migración v2 → v3 testeada;
   - cobertura ≥ 80 % en `forma-trace`, `forma-synth` y `forma-engine`;
   - CI en verde en los 3 SO;
   - el benchmark en modo C corre en CI como regresión de calidad (S, L y O; M y M-hand cada noche);
   - umbrales calibrados y documentados (§8.5).

### 8.5 Calibración de umbrales

Antes de consolidar los umbrales de §4 y el de estilo:
1. Se mide su distribución en S, M (geométrica, serif, sans y display), M-hand y O.
2. Se buscan los falsos positivos (defectos que pasan) y los falsos negativos (buenos que se rechazan), con la ayuda de los casos con verdad conocida.
3. Se documenta en `docs/qa/hito2-5/calibracion.md` cada valor, con las curvas, las razones y los ejemplos de aceptación y rechazo.
4. La versión de `QualityThresholds` sube si cambia algún valor.

Criterio: **ningún defecto topológico o de rasgo fino pasa sin detectarse**. Entre dos valores que cumplen eso, se elige el que menos homogeneiza (el que más conserva la irregularidad en O).

## 9. Riesgos

| Riesgo | Mitigación |
|---|---|
| Marching squares es sensible al ruido JPEG y da bordes rugosos | Estimar el ruido; presupuesto de fidelidad en px; modo `clean`; el benchmark M con JPEG lo mide |
| La agrupación une letras vecinas | Programación dinámica con restricciones duras, solo con layout, solo dentro de la fila; el margen de ambigüedad deriva al agente; casos de prueba en L |
| La síntesis produce glifos correctos pero sin alma | Derivar de piezas reales primero; umbral de estilo; `needs_design` en lugar de crear; revisión obligatoria |
| `detail: summary` por defecto rompe a un cliente que esperaba listas completas | Campos conservados; `*_truncated`; documentado; `detail: "full"` |
| El ahorro de tokens depende del cliente | Se mide por cliente; no se promete |
| Una transacción enorme en `font_from_image` | Límites existentes; si se superan, el informe parte en etapas reanudables |

## 10. Preguntas abiertas
Resueltas en §11.

## 11. Decisiones del responsable (2026-10-18)
1. **Informes:** se aprueba `.forma-reports/`, identificados por documento, revisión y ejecución, sin sobrescritura, acotados y prescindibles. Lo esencial (procedencia, revisión y métricas por glifo) vive en el documento (§5.4).
2. **Umbrales:** 0,75 de estilo y 0,75 px de Hausdorff p95 se aprueban como **iniciales y experimentales**, con calibración obligatoria (§8.5), registro de razones, puertas conjuntivas, `needs_review` ante ambigüedad y sin homogeneizar (§3, §4).
3. **Corpus:** se aprueban Inter, EB Garamond y Fraunces, versionadas sin modificar con licencia, atribución, procedencia y versión exacta. Se añaden casos orgánicos, irregulares, manuscritos y de geometría compleja (M-hand y O). La fidelidad geométrica y la calidad tipográfica se evalúan por separado.
4. **Ejecución:** por bloques de dependencias técnicas, no un subagente por tarea. Subagentes solo para trabajo realmente independiente, revisión independiente en los bloques críticos y pruebas después de cada bloque.
5. **Benchmarks:** la línea base sale de las transcripciones existentes; las pruebas con agentes tienen presupuesto; las metas de < 2 min y < US$ 0,50 son experimentales (§8.2).
6. **Formato v3:** migración sin pérdida, determinista y con respaldo; procedencia y revisión por glifo; los cambios geométricos se distinguen de los de métricas y metadatos; las aprobaciones no se invalidan sin cambio de diseño; se mantienen las garantías de historial, transacciones y concurrencia (§5.4).
7. **Seguridad:** ningún glifo sintetizado de baja confianza se aprueba automáticamente (§5.5).
8. **Consultar al responsable** ante cualquier decisión que comprometa la fidelidad tipográfica, la compatibilidad de documentos o la seguridad.

## 12. Enmiendas de la implementación

Lo que la implementación cambió respecto de esta especificación. El detalle y las razones están en `docs/qa/hito2-5/decisiones.md` (el número entre paréntesis).

1. **`layout` es obligatorio en `font_from_image`** (34). El modo `"auto"` de §5.2 no se implementó: sin layout, la asignación de letras sería adivinar.
2. **Alcance de las etapas de espaciado y kerning** (45, 54). Solo actúan sobre los glifos que hace la llamada. El kerning se limita a las letras trazadas y compuestas, y solo reemplaza pares entre dos glifos de la llamada. Los sintetizados se kernean tras su revisión.
3. **Las etapas respetan los toolsets deshabilitados** (55); el perfil de herramientas no las desactiva.
4. **Exportación tras la canalización** (35, 46):
   - omite los archivos existentes;
   - espera mientras haya glifos con `needs_look` o sintetizados pendientes;
   - pasa por `Engine::call`.
5. **Puertas de calidad** (24, 47, 50):
   - la desviación de ajuste es el percentil 99, no el máximo, a la espera de la decisión del responsable (48);
   - un rasgo encontrado en un ráster grueso se vuelve a medir a resolución completa;
   - una región junto al borde solo se excusa si no es más profunda que el presupuesto de ajuste;
   - el detalle de menos de una unidad de fuente es motivo de revisión.
6. **Síntesis** (38–44, 51–57):
   - **Kit de estilo:**
     - una sola clase de final (plano, redondo o bola);
     - medida de la forma del final (`terminal_fill`);
     - dispersión del trazo entre glifos.
   - **Recetas como datos** (`unicase-es` v1, con puntos unidos por diseño).
   - **Control de estructura** (piezas, contrahuecos, separación, tinta fuera de la caja, tamaño de los diacríticos).
   - **Autoprueba de reproducción** (umbral 0,9): por debajo, todo glifo dibujado por esqueleto pide una mirada.
   - **Procedencia sintetizada** con `strategy` y `sources` por nombre.
7. **Benchmark** (Task 19):
   - el modo C corre `font_from_image` y separa fidelidad y tipografía;
   - la puerta de regresión (S, L, O en cada push; M y M-hand nocturno) compara con `baseline.json`;
   - la irregularidad conservada solo cuenta los dibujos irregulares cuya imagen no difumina la tinta.
