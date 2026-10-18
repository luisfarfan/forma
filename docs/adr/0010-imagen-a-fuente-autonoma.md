# ADR 0010: Imagen → fuente autónoma: operación de alto nivel en el motor, procedencia y revisión en el documento (formato v3), síntesis por recetas con rechazo, e informes en archivo

- Estado: aceptada (Hito 2.5)
- Fecha: 2026-10-18
- Referencias:
  - `docs/superpowers/specs/2026-10-18-hito2-5-imagen-a-fuente-autonoma-design.md` (§3–§8 y §11, decisiones del responsable);
  - `docs/superpowers/plans/2026-10-18-hito2-5-imagen-a-fuente-autonoma.md`;
  - ADR 0002 (registro único), ADR 0004 (formatos), ADR 0005 (planes y tokens), ADR 0009 (trazado).

## Contexto

El Hito 2 convierte la imagen de un alfabeto en una fuente completa, pero la auditoría del 2026-10-18 midió que casi todo el costo estaba fuera del motor.

**Medición del Alphabetical:**

| | Valor |
|---|---|
| Tiempo con el agente | 986 s |
| Tiempo del motor local | ~1,2 s |
| Turnos | 173 |
| Costo | 6,94 USD |

**Dónde se fue:**
- **~70 % de los turnos:** diseñar 52 glifos escribiendo coordenadas y revisando 77 previews de uno en uno.
- **El resto:**
  - errores de máscara y topología (la ranura cerrada de la E y la H de dos piezas), que el agente tuvo que corregir a mano;
  - respuestas largas que el agente vuelve a leer en cada turno.

El responsable fijó la prioridad, en este orden:
1. calidad tipográfica y fidelidad;
2. automatización;
3. tokens;
4. velocidad.

## Decisión

### 1. `font_from_image`: una operación de alto nivel en el registro de comandos

La operación es un `CommandSpec` más de `forma-engine`, sin lógica en las fachadas (ADR 0002). Orquesta las etapas existentes (trazado, síntesis, composición, espaciado, kerning, validación) sobre un único `Steps`.

Con eso queda:
- **una transacción**, con etiqueta propia y deshacible con un `history_undo`;
- **un plan** bajo la política `prompted`, con revisiones por entidad y `REVISION_CONFLICT`;
- **las herramientas granulares intactas**, para los casos que necesitan intervención.

### 2. Procedencia y revisión dentro del documento (formato v3)

`Glyph.provenance` (opcional) registra:
- el origen: `traced`, `synthesized`, `composed` o `manual`;
- el estado de revisión: `pending`, `approved` o `rejected`;
- si hubo edición posterior (`edited`);
- el hash del diseño aprobado;
- la versión de umbrales con la que se juzgó;
- el detalle (imagen y candidatos, IoU, o receta y puntuación de estilo).

**Reglas:**
- **Solo un cambio de diseño invalida una aprobación:**
  - lo invalidan los contornos, nodos, componentes y sus transformaciones;
  - no lo invalidan las métricas (avance, márgenes, kerning) ni los metadatos (nombre, unicodes, exportación);
  - mover todo el contorno de lado, como hacen los márgenes, no es un cambio de diseño.
- **La regla vive en `forma-edit` (`apply`)**, así que vale para cualquier fachada y para el historial.
- **Un glifo rechazado** se conserva, pero no se exporta (`Glyph::is_exported`).
- **El formato pasa a 3.** La migración v1/v2 → v3 es pura y determinista: los campos nuevos son opcionales y nada se reescribe.
- **Respaldo:** la primera vez que se guarda en v3 un documento leído en v1 o v2, se copia intacto antes a `<archivo>.v<n>.bak`, una sola vez y sin sobrescribir. Es la vía para volver atrás, porque una versión anterior de FORMA no lee v3.
- **`edit_batch` no expone `set_provenance`.** La revisión pasa siempre por `glyph_review`.

### 3. Síntesis por recetas, con rechazo

Los caracteres que faltan se crean desde la morfología real de la fuente:
- primero se derivan de piezas existentes (el `0` de la `O`, el punto de una bola terminal, el guion del travesaño);
- si no hay piezas, se usan esqueletos propios vestidos con el perfil de grosor, los terminales, las uniones y la irregularidad medidos.

**Puntuación de estilo:** un glifo se crea solo si la puntuación total llega al umbral experimental y **ningún componente baja de 0,5**. Si no llega, queda `needs_design` y no se crea.

**Sin aprobación automática:** ningún glifo sintetizado se aprueba solo, en ninguna política. Aprobarlo exige un `glyph_review`.

**Licencia:** las recetas y los esqueletos son datos versionados, originales, bajo la licencia del repositorio.

### 4. Informes en archivo, diagnósticos y prescindibles

**Respuestas MCP compactas:** `detail: "summary"` por defecto en las herramientas con listas largas. Llevan totales, excepciones y `report_path`.

**El informe:**
- **Ubicación:** `.forma-reports/<doc-id>/r<rev>-<tool>-<UTC>-<n>.json`.
- **Formato:** JSON canónico, creado con apertura exclusiva, así que nunca sobrescribe un informe anterior.
- **Cabecera:** documento, revisión, herramienta, hash de argumentos, versión del motor y versión de umbrales.
- **Retención:** 20 informes por documento y herramienta, con un tope de 20 MB por documento. Sin píxeles ni contornos completos.
- **Escritura en herramientas de lectura:** también se escribe desde herramientas `READ`, porque un informe es un artefacto de auditoría, no un cambio del documento. En modo `--read-only` no se escribe nada.
- **Prescindible:** borrar `.forma-reports/` no pierde información, porque lo esencial (procedencia, revisión y métricas por glifo) vive en el documento.

### 5. Umbrales experimentales y versionados

Los umbrales de fidelidad (IoU, Hausdorff, topología, rasgos finos, G1) y el de estilo:
- viven en estructuras versionadas;
- se registran en cada informe y en cada procedencia;
- se calibran con un corpus geométrico, serif, sans, orgánico y manuscrito antes de consolidarlos.

Las puertas son conjuntivas: un IoU alto no compensa una contraforma perdida. Ante ambigüedad, el glifo queda `needs_review`.

## Alternativas descartadas

- **Seguir con herramientas granulares y mejores instrucciones al agente.** No elimina el costo dominante: dibujar coordenadas y revisar glifo por glifo.
- **Reconstrucción por Fourier de los contornos.** Produce oscilación (Gibbs) en las esquinas, su salida no es Bézier y no resuelve ninguno de los errores reales: ranuras cerradas y letras de varias piezas.
- **OCR o un modelo generativo para reconocer o dibujar glifos.** Es una dependencia externa, sin garantías de fidelidad ni de licencia, y viola "sin modelos nuevos para la geometría".
- **Plantillas genéricas parametrizadas solo por el grosor.** Homogeneizan la fuente y producen glifos ajenos a su estilo.
- **Procedencia fuera del documento** (en los informes o en un archivo lateral). Se perdería al mover o versionar el documento.
- **Invalidar la aprobación ante cualquier cambio.** El espaciado y el kerning automáticos anularían cada revisión.

## Consecuencias

- **Contrato `tools/list`:** cambio aditivo (5 herramientas nuevas, un conjunto `synthesis`, argumentos opcionales). Las respuestas de lista pasan a `summary` por defecto, un cambio de comportamiento compatible a nivel de schema que se documenta en `docs/qa/hito2-5/decisiones.md`.
- **Documentos:** un documento guardado en v3 ya no lo abre una versión anterior; el respaldo `.v<n>.bak` cubre la vuelta atrás.
- **Calidad:** el benchmark en modo C (sin IA) corre en CI como regresión de calidad.
- **Agentes:** el agente pasa de dibujar a supervisar. Su costo se mide por cliente y no se promete.
