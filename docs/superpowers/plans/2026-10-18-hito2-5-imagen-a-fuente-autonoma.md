# Hito 2.5 — De una imagen a una fuente con mínima intervención de IA — Implementation Plan

> **For agentic workers:** execute by **blocks** (see "Ejecución por bloques"), not one subagent per task. That is the owner's decision of 2026-10-18. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** FORMA convierte un alfabeto dibujado a mano en una fuente fiel, editable y válida con una sola operación de alto nivel. La IA solo supervisa, resuelve ambigüedades y aprueba. La mejora se mide contra el Hito 2 en calidad, tokens, costo y tiempo, por cliente.

**Architecture:**
- **Medir primero:** corpus propio (S, M, L) y analizador de transcripciones de agentes, y la línea base del modo A antes de tocar el motor.
- **Respuestas compactas:**
  - `detail` + informe en archivo;
  - hojas de contacto (`preview_sheet`);
  - perfil `image-font`;
  - output schemas opcionales.
- **Procedencia en el modelo (formato v3)** y `glyph_review`.
- **`font_from_image` en `forma-engine`:** reutiliza las funciones `*_commands` existentes sobre un único `Steps`, es decir, una transacción y un plan.
- **`forma-trace`:**
  - extractor subpíxel (marching squares) con presupuesto de fidelidad en px de origen;
  - validación topológica y de rasgos finos;
  - agrupación de piezas por layout.
- **Crate nuevo `forma-synth`:** kit de estilo, recetas como datos y puntuación de estilo que rechaza antes que crear un glifo mediocre.
- **Crate de desarrollo `forma-bench`:** corpus y modo C (sin IA) en CI.

**Tech Stack:**
- Rust 1.99.1 y las dependencias de los Hitos 1 y 2.
- Nueva, solo de desarrollo y solo en `forma-bench`: `skrifa`, la versión cuyo `read-fonts` coincide con el `read-fonts = "0.43.3"` del workspace (`MIT OR Apache-2.0`, mantenida por Google Fonts). Se fija exacta en la Task 2 tras verificarla en docs.rs.
- Python de la stdlib para el analizador de transcripciones. `fontTools` del venv de oráculos para extraer la verdad de las fuentes OFL.

**Spec:** `docs/superpowers/specs/2026-10-18-hito2-5-imagen-a-fuente-autonoma-design.md` (autoridad), junto con `CLAUDE.md`, `docs/03-architecture.md`, `docs/07-mcp-ai-architecture.md` y la especificación del Hito 2 con sus enmiendas.

## Global Constraints

**Reglas heredadas**
- Rigen todas las del Hito 2: licencias, toolchain, `#![forbid(unsafe_code)]`, sin `unwrap()`/`expect()` en librería, `thiserror`, `cargo fmt`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo deny check licenses`, inglés en el código y español en `docs/`.
- `export PATH="$HOME/.cargo/bin:$PATH"`; los comandos `cargo` se ejecutan desde `core/`.
- Commits por tarea en la rama `hito2-5`. **Sin push ni PR** salvo que el responsable lo pida.

**Prioridad (de la especificación):** calidad > automatización > tokens > velocidad. Ninguna tarea relaja un umbral de calidad para ganar tiempo o tokens. Si un umbral de §4 no se puede cumplir, se informa como fallo y no se ajusta en silencio.

**Umbrales de calidad (§4): iniciales y experimentales**
- Viven en `forma_trace::QualityThresholds { version: 1, … }`.
- Se registran en los informes y en la procedencia, y se calibran en la Task 18b.
- Las puertas son conjuntivas. Ante ambigüedad: `needs_review`.
- `iou_mask` ≥ 0,97.
- `hausdorff_p95_px` ≤ 0,75 (bloquea); `hausdorff_max_px` ≤ 2 (aviso).
- `topology`: igualdad de componentes y huecos, con cada hueco emparejado con IoU ≥ 0,8 (bloquea).
- `thin_features`: 100 % de los rasgos con ancho local ≥ 1 px presentes (bloquea).
- G1 ≤ 3° en nodos `smooth` (bloquea).
- `fit_deviation_px` ≤ 0,5 en `faithful` y ≤ 1,0 en `clean`.
- Nivel de ruido con piso de 0,25 px.
- Reintentos ante fallo topológico, en este orden: isolínea 0,45 → 0,55 → presupuesto 0,25 px → extractor `edge-walk`.

**Agrupación de piezas (§5.3)**
- Solo con layout y solo dentro de una fila.
- Solo piezas vecinas, con separación < 0,8 × trazo mediano y solapamiento vertical ≥ 50 % de la menor.
- Penalización si el ancho del grupo supera 1,6 × la mediana de la fila.
- Ambigüedad: la segunda mejor partición cuesta < 1,15 × la mejor.

**Síntesis (§5.5)**
- Umbral de estilo inicial y experimental: 0,75.
- Además, **ningún componente < 0,5**.
- Ningún glifo sintetizado se aprueba automáticamente, en ninguna política.
- Pesos de la puntuación: grosor 0,30, terminales 0,20, curvatura 0,15, gris 0,15, proporciones 0,10, irregularidad 0,10.
- Todo glifo sintetizado nace con `provenance.source = synthesized` y `review = pending`.

**Formato v3**
- `FORMAT_VERSION = 3`; `READABLE_VERSIONS = [1, 2, 3]`; se escribe siempre v3.
- `Glyph.provenance: Option<Provenance>` con `#[serde(default, skip_serializing_if = "Option::is_none")]`.
- Migración v2 → v3 pura: no añade procedencia.

**Las 55 herramientas, en orden de registro**
- Las 50 del Hito 2, en su orden.
- Después: `preview_sheet`, `glyph_review`, `font_from_image`, `style_extract`, `glyph_synthesize`.

| Herramienta | Conjunto | Efecto | Salida |
|---|---|---|---|
| `preview_sheet` | `preview` | `READ` | Imagen + resumen, sin `structuredContent` |
| `glyph_review` | `glyphs` | `WRITE` | JSON |
| `font_from_image` | `trace` | `DESTRUCTIVE` | JSON |
| `style_extract` | `synthesis` (nuevo; `Toolset::ALL` pasa a 12) | `READ` | JSON |
| `glyph_synthesize` | `synthesis` | `DESTRUCTIVE` | JSON |

**Argumentos aditivos**
- `detail: "summary" | "full"`, por defecto `summary`, en: `spacing_auto`, `kerning_auto`, `kerning_get`, `kerning_check`, `geometry_check`, `geometry_fix`, `validate`, `image_trace`, `charset_check`.
- `candidates: [usize]` en `image_trace` (excluyente con `candidate`).
- `wrap: bool` en `preview_text`.
- `fidelity`, `extractor` y `isoline` en `image_trace`.

**Flags nuevas de `forma mcp`**
- `--profile image-font`.
- `--output-schemas full|none`, por defecto `full`.
- `--print-catalog-size`.

**Perfil `image-font`** (lista exacta, en orden de registro):
- `forma_capabilities`, `document_create`, `document_open`, `document_save`, `document_close`, `document_list`;
- `plan_commit`, `plan_discard`;
- `glyph_list`, `glyph_get`, `glyph_set_outline`, `contour_edit`, `nodes_edit`;
- `validate`, `export_font`;
- `preview_glyph`, `preview_text`;
- `history_list`, `history_undo`, `history_redo`;
- `image_inspect`, `image_trace`;
- `preview_sheet`, `glyph_review`, `font_from_image`, `glyph_synthesize`.

**Informes en archivo**
- Ruta: `<carpeta del documento>/.forma-reports/<doc-id>/r<rev>-<tool>-<UTC>-<n>.json`, sin sobrescritura y con retención (Task 5). Si el documento no tiene ruta, se usa `.forma-reports/` dentro de la primera raíz permitida.
- Los informes son diagnósticos y prescindibles. Lo esencial vive en el documento.
- JSON canónico (claves ordenadas).
- En modo `--read-only` no se escriben informes: `report_path: null` y `report_note`. Decisión registrada en ADR 0010: un informe es un artefacto de auditoría, no un cambio del documento.

**Límites**
- `preview_sheet`: ≤ 120 celdas por página, imagen ≤ 1600 × 1600 px, celdas de 96 a 256 px.
- `attention` en las respuestas: ≤ 50 entradas, con `attention_total` y `attention_truncated`.
- `font_from_image` respeta `MAX_BATCH_OPS` y `MAX_NODES`; si se superan, `LIMIT_EXCEEDED` con el detalle por etapa.

**Contrato**
- Cambios del snapshot de `tools/list`: aditivos (5 herramientas, 1 conjunto, argumentos opcionales, campos de salida y patrón de `Glyph`).
- Cambio de comportamiento compatible: `detail` por defecto recorta listas (se registra en `docs/qa/hito2-5/decisiones.md`).
- Regenerar el snapshot: `INSTA_UPDATE=always cargo test -p forma-mcp --test contract tools_list_snapshot`.
- Regenerar la documentación: `cargo run --manifest-path core/Cargo.toml -p forma-cli -- gen-docs docs/api`.

**Contenido de terceros**
- Las fuentes OFL del corpus M se versionan **sin modificar**, con su `OFL.txt` en `tests/fixtures/<nombre>/`. No se subsetean, por la cláusula de Reserved Font Name.
- Las imágenes del corpus M y L se generan por código y no se versionan.
- El alfabeto de la hermana del responsable **nunca** entra al repositorio ni a resultados publicados.

## Ejecución por bloques (decisión del responsable, 2026-10-18)

| Bloque | Tareas | Modo | Revisión independiente |
|---|---|---|---|
| **A. Fundamentos** | 1, 4, 5, 7 | Secuencial, en la sesión principal (comparten el registro, el modelo y el snapshot) | Sí: formato v3 y migración (compatibilidad de documentos) |
| **B. Medición** | 2, 3 | **Subagente en un worktree aislado, en paralelo con A** (crate nuevo y `tools/bench/`, sin código compartido) | No; tests propios |
| **C. Trazado fiel** | 11, 12, 13 | Secuencial (comparten `forma-trace`) | **Sí** (fidelidad y topología) |
| **D. Pipeline** | 6, 8, 9, 10 | Secuencial | **Sí** (transacción única, aprobaciones, seguridad) |
| **E. Síntesis** | 14, 15, 16, 17, 18 | Secuencial | **Sí** (fidelidad de estilo, no auto-aprobación) |
| **F. Calibración y cierre** | 18b, 19, 20, 21 | Secuencial; corridas de agentes con presupuesto | Revisión final de toda la rama |

**Reglas de los bloques:**
- **Pruebas:**
  - al cerrar cada bloque: `cargo test --workspace --locked` + clippy + el modo C disponible;
  - dentro de un bloque, solo los tests del crate tocado;
  - sin recompilaciones completas innecesarias.
- **Revisiones:** un solo revisor por bloque crítico, sobre el diff del bloque. Si el revisor pide cambios, se corrigen y se re-revisa solo lo corregido.
- **Consultar al responsable** (y no decidir solo) ante cualquier decisión que comprometa la fidelidad tipográfica, la compatibilidad de documentos o la seguridad.

## Review Focus

1. **Una fila con más piezas que caracteres que NO deben unirse** (por ejemplo, `i` con punto en otra fuente, o un acento suelto): la agrupación solo une si se cumplen las restricciones duras. Si queda una pieza sobrante, va a `attention`; nunca se descarta en silencio ni se une con la vecina. Test en la Task 13 (`stray_piece_is_reported_not_merged`).
2. **Ranura que desaparece al binarizar pero existe en la cobertura suave** (la E): el extractor subpíxel la conserva. Si no la conserva, la topología lo detecta, y un IoU ≥ 0,97 no basta para aprobar. Tests en las Tasks 11 y 12 (`slit_1px_preserved`, `high_iou_missing_hole_fails`).
3. **Documento con cambios de otro actor durante `font_from_image`:** el comando falla con `REVISION_CONFLICT` sin aplicar nada, y un `history_undo` después de un éxito revierte todo el pipeline de una vez. Test en la Task 9 (`concurrent_edit_conflicts`, `single_undo_reverts_pipeline`).
4. **Editar a mano un glifo aprobado:** su revisión vuelve a `pending` y conserva el origen. Un glifo `rejected` no se exporta, y un compuesto que lo usa da error de validación. Tests en la Task 4.
5. **Fuente sin material para derivar** (solo 5 letras trazadas, sin O ni bolas): la síntesis devuelve `needs_design` con el motivo, no crea glifos de puntuación genéricos, y el pipeline sigue y exporta solo si se permite lo pendiente. Test en la Task 18 (`sparse_font_yields_needs_design`).

---

## Parte 0 — Decisiones

### Task 1: ADR 0010, roadmap y registro de decisiones

**Files:**
- Create: `docs/adr/0010-imagen-a-fuente-autonoma.md`, `docs/qa/hito2-5/decisiones.md`
- Modify: `docs/04-roadmap.md` (Hito 2.5 entre H2 y H3), `CLAUDE.md` (línea de estado), `docs/07-mcp-ai-architecture.md` (perfiles, `detail`, informes en archivo, output schemas opcionales)

**Interfaces:**
- Produces: ADR 0010 con:
  - la operación de alto nivel en el motor;
  - el modelo de procedencia y revisión;
  - el formato v3;
  - la síntesis por recetas con rechazo;
  - los informes en archivo (también en herramientas `READ`, salvo `--read-only`);
  - `detail` por defecto `summary`.

  Cada punto lleva sus alternativas descartadas (Fourier, OCR, modelo generativo, plantillas genéricas) y consecuencias.

- [ ] **Step 1:** Redactar el ADR y las modificaciones, en español, citando §3–§8 de la especificación.
- [ ] **Step 2:** Commit: `docs: ADR 0010 and milestone 2.5 in the roadmap`.

## Parte 1 — Medir y compactar (Fase 1)

### Task 2: Crate `forma-bench`: corpus S, M y L y modo C

**Files:**
- Create:
  - `core/crates/forma-bench/{Cargo.toml,src/lib.rs,src/corpus.rs,src/render_font.rs,src/degrade.rs,src/hard_cases.rs,src/metrics.rs,src/bin/forma-bench.rs}`
  - `tests/fixtures/{inter,eb-garamond,fraunces,patrick-hand}/` (TTF estáticos sin modificar + `OFL.txt` + `SOURCE.md` con autoría, URL, versión exacta y SHA-256)
  - `tools/bench/ground_truth.py`
- Modify: `core/Cargo.toml` (miembro; `skrifa` solo en este crate), `deny.toml` si hace falta.
- Test: `core/crates/forma-bench/tests/corpus.rs`

**Interfaces:**
- Produces:
  - `pub struct CorpusCase { pub id: String, pub level: Level, pub png: Vec<u8>, pub layout: Vec<String>, pub truth: Option<Truth> }`, con `Level::{S, M, L}`.
  - `pub struct Truth { pub glyphs: BTreeMap<String, TruthGlyph> }`; cada `TruthGlyph` lleva su `BezPath` en px de imagen, su número de huecos y su `px_rect`.
  - `Level::{S, M, MHand, L, O}`.
  - `pub fn corpus(level: Level) -> Result<Vec<CorpusCase>, BenchError>`. Es determinista: semillas fijas y sin reloj.
  - **M-hand:** igual que M, con Patrick Hand.
  - **O** (orgánico y complejo, generado por código, original), con id estable:
    - `hand_pressure`: esqueletos con presión variable y temblor;
    - `rough_brush`: borde rugoso;
    - `ink_bleed`;
    - `bulb_organic`: terminales de bola irregulares, bamboleo amplio;
    - `stencil`: muchas piezas por letra;
    - `inline_outline`: huecos dentro de huecos;
    - `complex_counters`: muchas contraformas y ranuras.
  - Cada caso O registra su `irregularity_amplitude_px` conocida en `Truth`.
  - **M:** las 26 mayúsculas de cada fuente OFL en 3 filas (9/9/8), renderizadas con `skrifa` y `forma-render`. Altura de mayúsculas 60, 110 y 200 px, y degradaciones `{none, jpeg q70, gauss σ=1.5 + ruido 4 %, rotación 1,5°}`.
  - **L:** casos con id estable:
    - `multi_piece_h`
    - `slit_1px`, `slit_2px`, `slit_3px`
    - `small_counter`
    - `organic_wobble`
    - `uneven_light`
    - `decorative_row`
    - `touching_pair`
    - `stray_dot`
    - `i_dot`
  - `forma-bench run --mode c --level s|m|l --out <dir>`: crea el documento, ejecuta el pipeline del modo C (al principio, la secuencia granular actual; desde la Task 10, `font_from_image`) y escribe `results.json` con las métricas de §8.3 que no dependen del agente, incluidos los tiempos por etapa.
  - `tools/bench/ground_truth.py <ttf> > truth.json`: avances, márgenes y kerning aplanado de GPOS (pares de glifos) con fontTools.

- [ ] **Step 1: Tests que fallan:**
  - `corpus_is_deterministic`: dos llamadas dan bytes idénticos.
  - `m_has_three_fonts_times_sizes_times_degradations` (36 casos).
  - `l_ids_are_stable` (lista exacta).
  - `truth_hole_counts`: O = 1, B = 2, H = 0, A = 1 en Inter.
  - `o_cases_have_known_irregularity`
  - `inline_outline_nested_holes`
  - `ground_truth_py_runs_on_inter`: desde la CLI, con el venv de oráculos; se omite con un motivo si no hay venv.
- [ ] **Step 2:** Implementar. Verificar en docs.rs la API de `skrifa` (`OutlineGlyphCollection`, `DrawSettings`, `OutlinePen`) antes de usarla y fijar la versión exacta.
- [ ] **Step 3:** `cargo test -p forma-bench` → PASS. `cargo deny check licenses` → PASS.
- [ ] **Step 4:** Commit: `feat(bench): deterministic S/M/L corpus with OFL ground truth and mode C runner`.

### Task 3: Analizador de transcripciones y línea base del modo A

**Files:**
- Create:
  - `tools/bench/agent_metrics.py`
  - `tools/bench/test_agent_metrics.py`
  - `tools/bench/fixtures/` (transcripciones sintéticas mínimas escritas a mano, **no** reales)
  - `tools/bench/prompts/{mode_a.txt,mode_b.txt}`
  - `tools/bench/run_agent.sh`
  - `docs/qa/hito2-5/linea-base.md`
- Modify: `core/crates/forma-cli/src/cli.rs` (`forma mcp --print-catalog-size`)

**Interfaces:**
- Produces:
  - `agent_metrics.py <transcript> --client claude-code|codex --engine-report <dir>` imprime un JSON con:
    - `turns`, `duration_s`, `engine_s`, `agent_s`;
    - `calls{tool: n}`, `arg_bytes`, `result_bytes`, `images`;
    - `tokens{input, cache_read, cache_write, output, reasoning}`;
    - `cost_usd` (el reportado, o calculado con `PRICES` versionado en el script, con su fecha) y `cost_source`.
  - Si un campo no existe en el formato de un cliente, sale `null` con `notes[]`; nunca se inventa.
  - `run_agent.sh <client> <mode> <case> <rep>` lanza el cliente con su servidor MCP (política `prompted` para B; `auto` para A, como en el Hito 2) y guarda la transcripción en `bench-out/` (ignorado por git).
  - `forma mcp --print-catalog-size [--profile …] [--toolsets …] [--output-schemas …]` imprime `{tools, chars_total, chars_input_schemas, chars_output_schemas, chars_descriptions, est_tokens}` y termina sin servir.

- [ ] **Step 1: Tests que fallan:**
  - `test_claude_stream_json_counts`: tokens, caché, turnos y costo de la transcripción sintética.
  - `test_codex_json_counts`.
  - `test_missing_fields_are_null_with_note`.
  - `catalog_size_reports_output_schema_share`: en la CLI, el total de 50 herramientas coincide con la suma de las partes.
- [ ] **Step 2:** Implementar y pasar los tests (`python -m unittest tools/bench/test_agent_metrics.py`).
- [ ] **Step 3 (controlador, local): línea base desde las transcripciones existentes.** No se repite el flujo antiguo.
  - Alphabetical: `~/fonts/hermana/claude.jsonl`, que **no se copia al repo**.
  - Specimen del Hito 2 con Claude Code.
  - Transcripciones de Codex del Hito 1, solo para el costo del catálogo.
  - **Única corrida nueva:** modo A con Codex sobre S, con tope de turnos, porque no hay una transcripción de Codex de imagen a fuente.

  Escribir `linea-base.md` con las tablas y el tamaño del catálogo por cliente.
- [ ] **Step 4:** Commit: `feat(bench): agent transcript metrics, catalog size and mode A baseline`.

### Task 4: Procedencia, formato v3 y `glyph_review`

**Files:**
- Modify:
  - `forma-model/src/glyph.rs` (+ `provenance.rs`);
  - `forma-edit` (comando `SetProvenance` y reglas de edición);
  - `forma-store/src/parts.rs` (v3 y migración);
  - `forma-validate/src/model_checks.rs`;
  - `forma-compile` (excluir `rejected`);
  - `forma-engine/src/{registry.rs,tools/glyphs.rs,tools/trace.rs}`.
- Test:
  - `forma-store/tests/migrate_v3.rs`
  - `forma-engine/tests/review_tools.rs`
  - `forma-edit` unit tests

**Interfaces:**
- Produces (en `forma_model`):
  - `pub struct Provenance { pub source: Source, pub review: Review, pub edited: bool, pub approved_geometry: Option<String /*hash*/>, pub thresholds_version: u32, pub detail: ProvenanceDetail }`;
  - `#[non_exhaustive] pub enum Source { Traced, Synthesized, Composed, Manual }`;
  - `#[non_exhaustive] pub enum Review { Pending, Approved, Rejected }`;
  - `#[non_exhaustive] pub enum ProvenanceDetail { Traced { image_sha256: String, candidates: Vec<usize>, px_rect: [f64; 4], iou_mask: f64, hausdorff_p95_px: Option<f64>, topology_ok: Option<bool> }, Synthesized { recipe: String, recipe_version: u32, sources: Vec<GlyphId>, style_score: f64 }, None }`.
  - `serde` en `snake_case`, con `JsonSchema`.
- `forma_edit::Command::SetProvenance { glyph: GlyphId, provenance: Option<Provenance> }`: toca la entidad del glifo y no es destructivo.
- **Regla en `forma-edit`:** se distinguen los cambios **geométricos** de los de métricas y metadatos.
  - **Geométricos:** contornos, nodos, componentes y sus transformaciones, quitar solapamientos, trazos y reemplazar una capa. Sobre un glifo con procedencia: `edited = true`, y si estaba `Approved` o `Rejected`, pasa a `Pending` en la misma transacción. El origen no cambia.
  - **Métricas** (avance, márgenes, kerning) y **metadatos** (nombre, unicodes, exportación): no tocan la revisión.
- `pub fn geometry_hash(glyph: &Glyph) -> String`: SHA-256 del JSON canónico de las capas sin métricas. Se guarda al aprobar.
- **Respaldo de migración** (en `forma-store`): al guardar en v3 un documento leído como v1 o v2, primero se escribe atómicamente `<ruta>.v<n>.bak`, o la carpeta `.formapackage` copiada, si no existe.
- **`image_trace`** escribe `Provenance { Traced, Pending, Traced{…} }` (en el Hito 2 no escribía nada). `hausdorff` y `topology` quedan en `None` hasta la Task 12.
- **`glyph_review`** (`Glyphs`, WRITE): `Mutation<{ glyphs: Vec<GlyphRef> (1..=500), decision: "approve" | "reject" }>` → `MutationResult` + `reviewed: usize`. Un glifo sin procedencia da `INVALID_ARGUMENT`.
- **`validate`:**
  - `glyph.pending_review` (WARN) por cada `Synthesized`/`Traced` en `Pending`;
  - `glyph.approval_stale` (WARN) si el `geometry_hash` actual no coincide con `approved_geometry`;
  - `glyph.rejected_used_by_component` (ERROR).
- **Compilación:** `Rejected` se trata como `export = false`.

- [ ] **Step 1: Tests que fallan:**
  - `v1_package_migrates_to_v3`
  - `v2_package_reads_and_writes_v3`
  - `v2_forma_sqlite_migrates`
  - `v4_is_unsupported_version`
  - `editing_approved_glyph_geometry_resets_review_and_sets_edited`
  - `metrics_and_rename_keep_approval`
  - `undo_restores_approval`
  - `migration_writes_backup_once`
  - `migration_is_deterministic`: dos migraciones dan bytes idénticos.
  - `v1_v2_roundtrip_field_by_field`: no se pierde información.
  - `external_geometry_change_warns_hash_mismatch`
  - `review_approve_then_undo`
  - `reject_excludes_from_export`
  - `rejected_component_base_is_validate_error`
  - `trace_sets_traced_provenance`
  - `review_without_provenance_is_invalid_argument`
- [ ] **Step 2:** Implementar y actualizar el snapshot (aditivo: `glyph_review` y `provenance` en el schema de `Glyph`) y la documentación.
- [ ] **Step 3:** `cargo test --workspace --locked` → PASS.
- [ ] **Step 4:** Commit: `feat(model): glyph provenance and review state, format v3, glyph_review`.

### Task 5: `detail` e informes en archivo

**Files:**
- Create: `core/crates/forma-engine/src/tools/report.rs`
- Modify:
  - `tools/{metrics.rs,kerning.rs,geometry.rs,quality.rs,trace.rs,charset.rs}`
  - `forma-cli` (ayuda)
  - snapshot y documentación
- Test: `core/crates/forma-engine/tests/report_tools.rs`

**Interfaces:**
- Produces:
  - `#[derive(Default)] pub enum Detail { #[default] Summary, Full }` (`snake_case`, `JsonSchema`).
  - `pub(crate) fn write_report(engine: &Engine, doc: Option<DocHandle>, tool: &str, body: &impl Serialize) -> Result<ReportRef, ToolError>`.
  - `pub struct ReportRef { pub report_path: Option<String>, pub report_note: Option<String> }`: se aplana en los resultados con `#[serde(flatten)]`.
- **Ruta e identidad del informe:**
  - `.forma-reports/<doc-id>/r<rev>-<tool>-<UTC AAAAMMDDTHHMMSSZ>-<n>.json`, creado con `create_new`, así que nunca sobrescribe.
  - Cabecera `{doc_id, rev, tool, args_sha256, engine_version, thresholds_version, created_utc}`.
  - Retención: 20 por documento y herramienta, ≤ 20 MB por documento; se poda al escribir.
  - Un informe > 2 MB se recorta con `truncated: true`.
  - Sin píxeles ni contornos completos.
- **Comportamiento:**
  - En `summary`, las listas de las 9 herramientas se reducen a las **excepciones**:
    - `spacing_auto`: solo los glifos `clamped` u omitidos;
    - `kerning_auto`: totales y los 20 pares de mayor valor absoluto;
    - `kerning_get`: totales por grupo;
    - `kerning_check`: solo las colisiones;
    - `geometry_check` y `fix`: solo los problemas;
    - `validate`: solo ERROR y WARN;
    - `image_trace`: los glifos bajo umbral + estadísticas mínima, mediana y máxima de IoU;
    - `charset_check`: solo los faltantes.
  - Cada lista recortada lleva `*_total` y `*_truncated`, y el detalle completo se escribe con `write_report`.
  - En `full`, la respuesta es la del Hito 2 (sin cambios) + `report_path`.
  - El informe se registra en el log de auditoría (`audit.rs`) con su ruta.

- [ ] **Step 1: Tests que fallan:**
  - `summary_is_default_and_bounded`: con la fuente unicase de prueba, `spacing_auto` en `summary` da ≤ 2 KB.
  - `full_matches_hito2_shape`
  - `report_written_inside_allowed_root`
  - `reports_never_overwrite_same_second`
  - `retention_prunes_oldest`
  - `deleting_reports_loses_nothing`: el documento reabierto da los mismos resultados.
  - `read_only_server_writes_no_report`
  - `report_is_canonical_json`
  - `report_logged_in_audit`
- [ ] **Step 2:** Implementar.
- [ ] **Step 3:** `cargo test --workspace --locked` → PASS, y snapshot aditivo.
- [ ] **Step 4:** Commit: `feat(engine): summary/full detail with canonical report files`.

### Task 6: `preview_sheet` y `preview_text` con `wrap`

**Files:**
- Create: `core/crates/forma-render/src/sheet.rs`
- Modify: `forma-render/src/{lib.rs,text.rs}`, `forma-engine/src/tools/preview.rs`, registro, snapshot y documentación
- Test: `core/crates/forma-render/tests/sheet.rs`, `core/crates/forma-engine/tests/output_tools.rs`

**Interfaces:**
- Consumes: `Provenance` (Task 4).
- Produces:
  - `forma_render::sheet_png(font: &Font, cells: &[SheetCell], opts: &SheetOptions) -> Result<SheetImage, RenderError>`.
  - `SheetCell { glyph: GlyphId, label: String, badge: Badge, overlay: Option<Mask> }`.
  - `Badge::{Traced, Synthesized, Composed, Manual, NeedsReview, Rejected}`: un color y una letra por badge (T, S, C, M, !, ×), con una leyenda dibujada en la imagen.
  - `SheetImage { png: Vec<u8>, columns: u32, cell: u32 }`.
- **`preview_sheet`:** `{ doc, glyphs?: Vec<GlyphRef>, filter?: "all" | "traced" | "synthesized" | "composed" | "manual" | "pending" | "needs_review", page?: u32, cell?: u32 (96..=256), overlay?: { path, color?, tolerance? }, master? }` → PNG + `{ rev, page, pages, cells: [{name, source, review}], total }`.
  - Con `overlay`, la máscara del candidato registrado en `ProvenanceDetail::Traced` se superpone a la celda si el `image_sha256` coincide. Si no coincide, sale una nota.
- **`text_png_wrapped(font, text, height, max_width) -> …`:** `preview_text { wrap: true }` corta en varias líneas por palabras hasta 2048 px de ancho.

- [ ] **Step 1: Tests que fallan:**
  - `sheet_fits_budget_with_120_cells`
  - `sheet_pages`
  - `badges_follow_provenance`
  - `overlay_requires_matching_image`
  - `explicit_list_over_120_is_limit_exceeded`
  - `wrap_keeps_all_text`: ninguna letra recortada con el texto de la línea 4 de la prueba del Alphabetical, recreado con la fuente unicase de prueba.
  - `sheet_has_no_structured_content` (en forma-mcp).
- [ ] **Step 2–3:** Implementar; `cargo test --workspace --locked` → PASS.
- [ ] **Step 4:** Commit: `feat(preview): contact sheets with provenance badges; wrapped text previews`.

### Task 7: Perfil `image-font` y output schemas opcionales

**Files:**
- Modify: `forma-engine/src/registry.rs` (`pub fn profile(name: &str) -> Option<&'static [&'static str]>`), `forma-cli/src/cli.rs`, `forma-mcp/src/{server.rs,convert.rs}`
- Test: `core/crates/forma-mcp/tests/contract.rs`, `core/crates/forma-cli/tests/mcp_safety.rs`

**Interfaces:**
- Produces:
  - `--profile image-font` sirve exactamente la lista de Global Constraints. Es compatible con `--toolsets` (se intersecan) y con `--read-only`.
  - `--output-schemas none` omite `outputSchema` en `tools/list`; las respuestas siguen trayendo `structuredContent`.
  - Las instrucciones del servidor en el perfil `image-font` describen el flujo de §5.2 en ≤ 10 líneas, en inglés.

- [ ] **Step 1: Tests que fallan:**
  - `profile_image_font_exact_list`
  - `unknown_profile_is_cli_error`
  - `output_schemas_none_omits_but_still_structured`
  - `default_catalog_unchanged`: el snapshot por defecto no cambia en esta tarea.
  - Los dos modos del protocolo (`initialize` y `server/discover`).
- [ ] **Step 2–3:** Implementar; tests → PASS.
- [ ] **Step 4 (controlador):** `forma mcp --print-catalog-size` con el perfil por defecto, con `image-font` y con `image-font --output-schemas none`. Anotar en `linea-base.md`, como estimación.
- [ ] **Step 5:** Commit: `feat(mcp): image-font tool profile and optional output schemas`.

## Parte 2 — `font_from_image` (Fase 2)

### Task 8: Asignación por layout y etapas reutilizables

**Files:**
- Create: `core/crates/forma-trace/src/layout.rs`, `core/crates/forma-engine/src/tools/pipeline/{mod.rs,stages.rs}`
- Modify:
  - `forma-engine/src/tools/{trace.rs,metrics.rs,kerning.rs,components.rs,charset.rs}` (exponer las funciones de comandos que faltan, sin cambiar el comportamiento);
  - `forma-validate/src/charset.rs` (`glyph_name`).
- Test: `core/crates/forma-trace/tests/layout.rs`, `core/crates/forma-engine/tests/stages.rs`

**Interfaces:**
- Produces:
  - `forma_trace::layout::assign(analysis: &Analysis, layout: &Layout, extra: &[ExtraPiece]) -> LayoutPlan`.
  - `Layout::Rows(Vec<String>)` o `Layout::Auto(profile)`.
  - `LayoutPlan { assignments: Vec<PlannedAssignment { candidates: Vec<usize>, char: Option<char>, name: String, unicodes: Vec<u32>, row: usize }>, attention: Vec<Attention> }`.
  - **En esta tarea solo hay coincidencia exacta:** filas detectadas = filas del layout y piezas por fila = caracteres de la fila. Si no coincide, `attention` con `kind: "row_count_mismatch" | "piece_count_mismatch"`, la fila y lo esperado contra lo encontrado. Las filas que coinciden se asignan igual.
  - `forma_validate::charset::glyph_name(cp: u32) -> String`: nombres de producción del perfil `unicase-es` (los que ya usa la fuente de prueba del Hito 2) y `uniXXXX` para el resto. Si ya existe un equivalente, se reutiliza.
  - **Etapas** (`pub(crate)`, sobre `&mut Steps`, sin efectos fuera de `Steps`): `trace_stage`, `compose_stage`, `spacing_stage`, `kerning_stage`, `kerning_check_stage` y `charset_stage`.
    - Cada una devuelve `StageOutcome { name, status: Ok | Warn | Fail, counts: BTreeMap<String, u64>, ms: u64, attention: Vec<Attention>, detail: serde_json::Value }`.
    - Usan las funciones `*_commands` existentes. Las herramientas granulares pasan a llamarlas sin cambiar sus resultados.

- [ ] **Step 1: Tests que fallan:**
  - `layout_exact_assigns_in_reading_order` (S)
  - `layout_mismatch_reports_row`
  - `extra_index_from_end_names_brace`
  - `glyph_name_known_and_fallback`
  - `stages_equal_granular_tools`: sobre la fuente de prueba, aplicar las etapas da el mismo `Font` que llamar a las herramientas en secuencia.
  - Todos los tests existentes de herramientas siguen en verde (refactor sin cambio de comportamiento).
- [ ] **Step 2–3:** Implementar; `cargo test --workspace --locked` → PASS.
- [ ] **Step 4:** Commit: `refactor(engine): reusable pipeline stages; feat(trace): layout assignment`.

### Task 9: Comando `font_from_image`

**Files:**
- Create: `core/crates/forma-engine/src/tools/pipeline/font_from_image.rs`
- Modify: registro, `tools/plans.rs` (campo opcional `context` en la respuesta de plan), capacidades, snapshot y documentación
- Test: `core/crates/forma-engine/tests/font_from_image.rs`

**Interfaces:**
- Consumes: Tasks 4, 5 y 8.
- Produces:
  - `font_from_image` con los argumentos de §5.2 de la especificación: `Mutation<FontFromImageOpArgs>`.
  - `FontFromImageOpArgs`:
    - `path`, `layout: LayoutArg` (`Vec<String>` o `"auto"`), `extra`;
    - `profile` (por defecto `"unicase-es"`);
    - `fidelity` (por defecto `faithful`);
    - `stages: Stages` (por defecto todo en `true`, `export: None`);
    - `color`, `tolerance`, `master`, `detail`.
  - **Flujo del handler:**
    1. Leer y analizar la imagen.
    2. Calcular el `LayoutPlan`.
    3. Crear un `Steps` y ejecutar en orden: trace → (synthesize: no-op hasta la Task 18) → compose → spacing → kerning → kerning_check → charset.
    4. Validar sobre `steps.after`.
    5. `write_report`.
    6. `mutate(…)` con la etiqueta `font_from_image`.
    7. Si hubo commit y `stages.export`, exportar con las funciones de `export.rs`. Si el archivo existe, `export.skipped = "exists; use export_font"`. Si hay `pending`/`needs_review` sin `allow_pending`, `export.skipped = "pending_review"`.
  - **Resultado:** `FontFromImageResult` = `MutationResult` + `stages: Vec<StageSummary { name, status, counts, ms }>` + `attention` (≤ 50, con `_total` y `_truncated`) + `report_path` + `export: Option<ExportOutcome { written: Vec<String>, skipped: Option<String> }>` + `engine_ms`.
  - **Plan:** la respuesta de aprobación añade `context` (el mismo resumen compacto). Es un campo opcional nuevo en el schema de plan (aditivo).

- [ ] **Step 1: Tests que fallan:**
  - `s_end_to_end_auto_policy`: S produce glifos, espaciado, kerning y `validate` sin errores.
  - `prompted_policy_returns_single_plan_with_context`
  - `plan_commit_applies_everything_once`
  - `single_undo_reverts_pipeline` (Review Focus 3)
  - `concurrent_edit_conflicts` (Review Focus 3)
  - `dry_run_changes_nothing`
  - `export_skipped_when_pending`
  - `export_skipped_when_file_exists`
  - `limit_exceeded_reports_stage`
  - `response_under_4kb_on_s`
- [ ] **Step 2–3:** Implementar; `cargo test --workspace --locked` → PASS; snapshot aditivo.
- [ ] **Step 4:** Commit: `feat(engine): font_from_image pipeline as one transaction and one plan`.

### Task 10: Escenario MCP del modo B y paridad CLI

**Files:**
- Create: `core/crates/forma-cli/tests/mcp_scenario_hito2_5.rs`
- Modify: `core/crates/forma-cli/tests/cli.rs`, `forma-bench` (modo C usa `font_from_image`)

**Interfaces:**
- Consumes: Task 9.
- Produces: el escenario MCP en las dos eras del protocolo, con el perfil `image-font` y la política `prompted`:
  1. `document_create`
  2. `font_from_image` con `dry_run`
  3. `font_from_image` → plan
  4. `plan_commit`
  5. `preview_sheet` con `filter: "pending"`
  6. `glyph_review` con `approve`
  7. `export_font`
  8. `validate`

  Cuenta las llamadas (8) y la suma de bytes de respuesta. Paridad: `forma font-from-image --layout … --yes` produce el mismo documento.

- [ ] **Step 1: Tests que fallan:**
  - `scenario_b_both_protocol_eras`
  - `scenario_b_response_bytes_under_budget`: ≤ 24 KB de texto en total, sin contar el PNG.
  - `cli_font_from_image_matches_mcp`
- [ ] **Step 2–3:** Implementar; tests → PASS.
- [ ] **Step 4:** Commit: `test(mcp): milestone 2.5 scenario and CLI parity for font_from_image`.

## Parte 3 — Segmentación, máscara y topología (Fase 3)

### Task 11: Extractor subpíxel, ruido y presupuesto de fidelidad

**Files:**
- Create: `core/crates/forma-trace/src/{subpixel.rs,noise.rs}`
- Modify: `forma-trace/src/{vectorize.rs,cleanup.rs,candidates.rs}` (`TraceOptions { extractor, fidelity, isoline }`), `forma-engine/src/tools/trace.rs` (argumentos aditivos)
- Test: `core/crates/forma-trace/tests/subpixel.rs`

**Interfaces:**
- Produces:
  - `#[non_exhaustive] pub enum Extractor { Subpixel, EdgeWalk }`, por defecto `Subpixel`.
  - `pub enum Fidelity { Faithful, Clean }`.
  - `TraceOptions.isoline: f64`, por defecto 0,5 (rango 0,3–0,7).
  - `subpixel::contours(coverage: &GrayImage, rect: PxRect, isoline: f64) -> Vec<Vec<Point>>`: marching squares a resolución original, con interpolación lineal y desambiguación de las celdas silla por el promedio de la celda. Contornos cerrados y orientados (exteriores antihorarios en unidades de fuente).
  - `noise::edge_noise_px(contours) -> f64`: desviación típica del borde respecto a una recta en tramos rectos ≥ 12 px, con piso de 0,25.
  - El ajuste (esquinas + Schneider + simplificación) usa `budget_px` = 0,5 o 1,0 (según `fidelity`), convertido a unidades con la escala de la fila. **Nunca baja del nivel de ruido** en `clean` y **nunca suaviza por debajo del presupuesto** en `faithful`.
  - `TracedGlyph` gana `fit_deviation_px: f64` y `noise_px: f64`.

- [ ] **Step 1: Tests que fallan** (imágenes generadas por código):
  - `slit_1px_preserved` (Review Focus 2)
  - `slit_2px_preserved`
  - `circle_radius_error_under_0_25px`
  - `organic_wobble_kept_in_faithful`: amplitud del bamboleo conservada ≥ 80 %.
  - `organic_wobble_smoothed_in_clean_only_below_noise`
  - `jpeg_noise_estimate_in_range`
  - `edge_walk_still_available`
  - `s_iou_not_worse_than_hito2`: IoU mínimo de S ≥ el del Hito 2 (0,9853).
- [ ] **Step 2–3:** Implementar; `cargo test -p forma-trace` → PASS. Correr `forma-bench run --mode c --level s,l` y anotar las métricas antes y después en `decisiones.md`.
- [ ] **Step 4:** Commit: `feat(trace): subpixel marching-squares extraction with a pixel fidelity budget`.

### Task 12: Topología, rasgos finos, Hausdorff y reintentos

**Files:**
- Create: `core/crates/forma-trace/src/{topology.rs,quality.rs}`
- Modify: `forma-trace/src/{fidelity.rs,vectorize.rs}`, `forma-engine/src/tools/{trace.rs,pipeline/stages.rs}` (`needs_review`, procedencia completa)
- Test: `core/crates/forma-trace/tests/topology.rs`

**Interfaces:**
- Produces:
  - `topology::of_mask(mask: &[bool], w, h) -> Topology { components: usize, holes: Vec<Hole { area: usize, bbox: PxRect, mask: … }> }`: tinta con conectividad 8 y fondo con conectividad 4.
  - `thin_features(mask) -> Vec<ThinFeature { kind: Slit | Stroke, width_px: f64, at: (f64, f64) }>`: ancho local desde la transformada de distancia existente; se toman los mínimos locales del ancho ≥ 1 px y < 2,5 × el nivel de ruido + 2 px.
  - `quality::evaluate(candidate_mask, traced_outline, scale) -> QualityReport { iou_mask, hausdorff_p95_px, hausdorff_max_px, topology_ok, missing_holes, extra_holes, thin_ok, thin_missing: Vec<ThinFeature>, g1_violations, nodes_per_1000 }`.
  - `QualityReport::passes(&Thresholds) -> Result<(), Vec<Failure>>` con los umbrales de Global Constraints.
  - **En la etapa de trazado y en `image_trace`:** si un glifo falla, se reintenta en el orden fijado. Si sigue fallando, el glifo se crea igual, con `review = Pending` y `attention { kind: "needs_review", glyph, reasons: […], crop_px }`; el motivo es legible, como "falta 1 hueco" o "ranura de 1,4 px cerrada en (x, y)".
  - `ProvenanceDetail::Traced` se completa con `hausdorff_p95_px` y `topology_ok`.

- [ ] **Step 1: Tests que fallan:**
  - `high_iou_missing_hole_fails` (Review Focus 2): un contorno sin el hueco pequeño de una B tiene IoU ≥ 0,97 y aun así falla.
  - `extra_hole_fails`
  - `slit_closed_is_reported_with_location`
  - `retry_ladder_order`
  - `needs_review_glyph_created_pending`
  - `hausdorff_p95_on_circle`
  - `g1_violation_detected`
  - `l_small_counter_passes`
- [ ] **Step 2–3:** Implementar; tests → PASS. Correr el modo C en S, M y L y anotarlo.
- [ ] **Step 4:** Commit: `feat(trace): topology, thin-feature and Hausdorff gates with a retry ladder`.

### Task 13: Agrupación de piezas y `image_trace` con varios candidatos

**Files:**
- Modify: `forma-trace/src/layout.rs` (programación dinámica), `forma-trace/src/vectorize.rs` (trazar la unión de varias máscaras), `forma-engine/src/tools/trace.rs` (`Assignment.candidates`)
- Test: `core/crates/forma-trace/tests/layout.rs`, `core/crates/forma-engine/tests/trace_tools.rs`

**Interfaces:**
- Produces:
  - `layout::group_row(pieces: &[Candidate], k: usize, stroke_px: f64) -> Grouping`.
  - `Grouping { Unique(Vec<Vec<usize>>), Ambiguous { best, second, cost_ratio }, Infeasible { reason } }`.
  - **Programación dinámica:** sobre las piezas ordenadas por x, con particiones contiguas en exactamente `k` grupos.
    - Costo = Σ separaciones internas / trazo + Σ |ancho − mediana| / mediana + penalización 10 si ancho > 1,6 × mediana.
    - Las restricciones duras de Global Constraints hacen infinito el costo de un grupo que las viola.
  - `assign` llama a `group_row` cuando hay más piezas que caracteres en una fila con layout:
    - `Unique` → asignaciones con varios `candidates`;
    - `Ambiguous` → `attention { kind: "ambiguous_grouping", row, options }` y esa fila no se asigna;
    - `Infeasible`, menos piezas que caracteres o letras tocándose → `attention { kind: "touching_or_missing" }`.
  - `Assignment { candidate: Option<usize>, candidates: Option<Vec<usize>> }`: exactamente uno de los dos; si no, `INVALID_ARGUMENT`. Las piezas se trazan juntas, con la unión de sus máscaras, en la misma ubicación de la fila.

- [ ] **Step 1: Tests que fallan:**
  - `multi_piece_h_grouped`
  - `stray_piece_is_reported_not_merged` (Review Focus 1)
  - `i_dot_grouped_only_with_vertical_overlap_rule`
  - `ambiguous_row_goes_to_attention`
  - `never_merges_across_rows`
  - `touching_pair_reported_not_split`
  - `image_trace_candidates_and_candidate_exclusive`
  - `no_layout_no_merge`
- [ ] **Step 2–3:** Implementar; tests → PASS; snapshot aditivo. Correr el modo C en L y verificar 0 uniones accidentales.
- [ ] **Step 4:** Commit: `feat(trace): constrained layout grouping of multi-piece glyphs`.

## Parte 4 — Síntesis estilística (Fase 4)

### Task 14: Crate `forma-synth`: kit de estilo y `style_extract`

**Files:**
- Create: `core/crates/forma-synth/{Cargo.toml,src/lib.rs,src/error.rs,src/kit.rs,src/skeleton.rs,src/terminals.rs,src/curvature.rs,src/irregularity.rs}`, `forma-engine/src/tools/synthesis.rs`
- Modify: `core/Cargo.toml`, `forma-trace` (exponer `distance_transform` y el esqueleto como `pub`), registro (`Toolset::Synthesis`), snapshot y documentación
- Test: `core/crates/forma-synth/tests/kit.rs`

**Interfaces:**
- Produces:
  - `forma_synth::StyleKit::extract(font: &Font, master: MasterId, sources: &[GlyphId]) -> Result<StyleKit, SynthError>`. Las fuentes por defecto son los glifos `Traced` que no están `Rejected`.
  - `StyleKit`:
    - `stroke: WidthStats { median, p10, p90, contrast, axis_deg }`;
    - `terminals: Vec<Terminal { outline: BezPath (normalizado a ancho 1), width, angle_deg, source: GlyphId }>`;
    - `joins: JoinStats { pinch_ratio }`;
    - `bowls: Option<BowlStats { superellipse_n, overshoot }>`;
    - `corners: CornerStats { radius_median }`;
    - `irregularity: Irregularity { amplitude_px_units, spectrum: [f64; 8], organic: bool }`;
    - `proportions: Proportions { round_width, straight_width, diagonal_width, cap_height, lsb_median, rsb_median }`;
    - `color: f64` (tinta por em²).
  - `skeleton::of_glyph(font, glyph, master) -> Skeleton { strokes: Vec<SkelStroke { points: Vec<(Point, f64 /*width*/)> }>, junctions, endpoints }`.
  - `terminals`: recorte en el cuello, es decir, el primer punto desde el extremo donde el ancho vuelve a ±10 % de la mediana.
  - `style_extract` (`Synthesis`, READ): `{ doc, glyphs?, master? }` → `{ rev, kit }` resumido, con el kit completo en el informe.

- [ ] **Step 1: Tests que fallan:**
  - `kit_from_specimen_stroke_matches_glyph_measure`: ±2 %.
  - `terminal_catalog_recovers_bulbs`: en una letra generada con bolas, el radio ±5 %.
  - `superellipse_of_circle_is_2`
  - `organic_flag_on_wobble_off_on_inter`
  - `kit_requires_traced_sources`: si no hay glifos fuente, `SynthError::NoSources`.
- [ ] **Step 2–3:** Implementar; tests → PASS; cobertura de `forma-synth` ≥ 80 %.
- [ ] **Step 4:** Commit: `feat(synth): style kit extraction and style_extract`.

### Task 15: Operaciones de síntesis y formato de recetas

**Files:**
- Create: `core/crates/forma-synth/src/{ops.rs,recipe.rs,restroke.rs}`, `core/crates/forma-synth/recipes/unicase-es.json`, `core/crates/forma-synth/recipes/README.md` (licencia y versión)
- Test: `core/crates/forma-synth/tests/ops.rs`

**Interfaces:**
- Produces:
  - `Recipe { char: char, name: String, version: u32, strategies: Vec<Strategy> }`.
  - `Strategy::Derive { requires: Vec<String> /*glyph names*/, ops: Vec<Op> }` o `Strategy::Skeleton { skeleton: String, dress: Dress }`.
  - `#[non_exhaustive] enum Op`:
    - `ExtractSkeleton { glyph }`
    - `FitBox { width: WidthRef, height: HeightRef }`
    - `Affine { m: [f64; 6] }`
    - `Mirror { axis }`
    - `Restroke { profile: ProfileRef }`
    - `CutAtNeck { glyph, near: [f64; 2] }`
    - `AttachTerminal { at: EndRef, terminal: TerminalRef }`
    - `Union`
    - `Place { baseline_offset: HeightRef }`
  - Las referencias son relativas al kit (`cap_height × 0.5`, `stroke.median × 1.2`, etc.), nunca en unidades absolutas.
  - `restroke(skel, profile, kit) -> BezPath`: reutiliza `glyph_stroke` de `forma-geometry` con el ancho variable muestreado y los terminales del catálogo en lugar de `Bulb`.
  - `recipes/unicase-es.json`:
    - cubre los 52 caracteres no-letra de `unicase-es` (cifras, puntuación, símbolos y las 3 marcas);
    - estrategias `Derive` primero, según §5.5 (`zero`←O, `one`←I, `period`←bola, `comma`←bola+cola, `hyphen`←travesaño de H, `dollar`←S+trazo, etc.);
    - es trabajo original de FORMA, bajo la licencia del repositorio.

- [ ] **Step 1: Tests que fallan:**
  - `recipes_parse_and_cover_profile_non_letters`
  - `zero_from_o_keeps_o_profile`: el perfil de grosor de `zero` correlaciona ≥ 0,9 con el de `O`.
  - `period_reuses_terminal_outline`: Hausdorff normalizado al terminal del catálogo ≤ 0,05.
  - `hyphen_from_h_bar_thickness`
  - `ops_are_deterministic`
  - `recipe_refs_are_relative_only`: el validador de recetas rechaza unidades absolutas.
- [ ] **Step 2–3:** Implementar; tests → PASS.
- [ ] **Step 4:** Commit: `feat(synth): derivation operations and versioned unicase-es recipes`.

### Task 16: Esqueletos paramétricos con estilo transferido

**Files:**
- Create: `core/crates/forma-synth/src/{param.rs,dress.rs}`, `core/crates/forma-synth/skeletons/*.json` (esqueletos originales en unidades relativas)
- Test: `core/crates/forma-synth/tests/param.rs`

**Interfaces:**
- Produces:
  - `param::skeleton(name: &str, kit: &StyleKit) -> Result<Skeleton, SynthError>`: proporciones a partir de `kit.proportions`.
  - `dress(skel, kit, opts: DressOptions { irregularity: bool }) -> BezPath` aplica:
    - el perfil de grosor y el contraste según `axis_deg`;
    - los terminales del catálogo;
    - el pinch de las uniones;
    - la superelipse en las curvas;
    - la firma de irregularidad (solo si `kit.irregularity.organic`), determinista por semilla = hash del nombre del glifo.

- [ ] **Step 1: Tests que fallan:**
  - `dressed_stroke_width_matches_kit`: mediana ±5 %.
  - `contrast_axis_applied`
  - `no_irregularity_on_geometric_font`
  - `irregularity_amplitude_matches_kit`: ±20 %.
  - `skeletons_cover_strategy_fallbacks`: todo carácter cuya receta tiene `Skeleton` tiene su esqueleto.
- [ ] **Step 2–3:** Implementar; tests → PASS.
- [ ] **Step 4:** Commit: `feat(synth): parametric skeletons dressed with the measured style`.

### Task 17: Puntuación de estilo

**Files:**
- Create: `core/crates/forma-synth/src/score.rs`
- Test: `core/crates/forma-synth/tests/score.rs`

**Interfaces:**
- Produces:
  - `score(candidate: &BezPath, kit: &StyleKit, reference: &[GlyphId], font: &Font) -> StyleScore`.
  - `StyleScore { total, stroke, terminal, curvature, color, proportion, irregularity, reasons: Vec<String> }`: cada componente en [0, 1].
  - **Componentes:**
    - grosor: `exp(-max(0, |z| - 1))`, con z = (mediana − mediana del kit) / ((p90 − p10) / 2,56);
    - terminales: `1 − min(1, Hausdorff normalizado al terminal más cercano / 0,15)`;
    - curvatura: `exp(-|n − n_kit| / 0,5)` si hay curvas; si no, 1;
    - gris: `exp(-|color/color_kit − 1| / 0,15)`;
    - proporciones: `exp(-|ancho/ancho_ref − 1| / 0,2)`, con la referencia según la clase del carácter en la receta;
    - irregularidad: `exp(-|amp/amp_kit − 1| / 0,5)` si `organic`; si no, `exp(-amp / ruido)`.
  - `total` es la media ponderada con los pesos de Global Constraints. `reasons` explica cada componente < 0,6.
  - `StyleScore::accepts(&self, min_total) -> bool`: `total ≥ min_total` **y** todos los componentes ≥ 0,5.

- [ ] **Step 1: Tests que fallan:**
  - `true_glyph_scores_high`: en M, las cifras reales de Inter puntuadas contra el kit de sus mayúsculas dan ≥ 0,8 (calibración).
  - `generic_template_scores_low`: un `zero` geométrico con grosor fijo contra el kit orgánico da < 0,75.
  - `weights_sum_to_one`
  - `single_bad_component_rejects_despite_high_total`
  - `reasons_name_failing_components`
- [ ] **Step 2–3:** Implementar; tests → PASS. Si la calibración contradice el 0,75, **no se cambia el umbral**: se registra en `decisiones.md` y se consulta al responsable (pregunta abierta 2 de la especificación).
- [ ] **Step 4:** Commit: `feat(synth): style consistency score`.

### Task 18: `glyph_synthesize` e integración en `font_from_image`

**Files:**
- Modify: `forma-engine/src/tools/{synthesis.rs,pipeline/stages.rs,pipeline/font_from_image.rs}`, registro, snapshot y documentación
- Test: `core/crates/forma-engine/tests/synthesis_tools.rs`

**Interfaces:**
- Consumes: Tasks 14 a 17.
- Produces:
  - `glyph_synthesize` (`Synthesis`, DESTRUCTIVE): `Mutation<{ glyphs?: Vec<String /*names or chars*/>, profile?: String, min_style_score?: f64 (0..=1, por defecto 0,75), master? }>`. Por defecto sintetiza los faltantes del perfil.
  - Por carácter:
    1. Se prueban las estrategias en orden.
    2. Se elige la de mayor puntuación.
    3. Si `total ≥ min`, se crea el glifo con `Provenance { Synthesized, Pending, Synthesized { recipe, recipe_version, sources, style_score } }`.
    4. Si no, `attention { kind: "needs_design", glyph, best_score, reasons }`, el glifo **no se crea** y el boceto va al informe como SVG.
  - Las marcas combinantes se crean con avance 0, como en el Hito 2.
  - Resultado: `MutationResult` + `synthesized: [{name, score}]` (con límite) + `needs_design_total` + `attention` + `report_path`.
  - `synthesize_stage` en el pipeline (Task 9) sustituye al no-op.

- [ ] **Step 1: Tests que fallan:**
  - `synthesized_glyphs_are_marked_pending`
  - `below_threshold_not_created`
  - `sparse_font_yields_needs_design` (Review Focus 5)
  - `pipeline_with_synthesis_one_transaction`
  - `preview_sheet_filter_synthesized`
  - `existing_glyph_replacement_needs_plan`
  - `auto_policy_never_approves_synthesized`
- [ ] **Step 2–3:** Implementar; `cargo test --workspace --locked` → PASS; snapshot aditivo (55 herramientas).
- [ ] **Step 4:** Commit: `feat(engine): glyph_synthesize with provenance and refusal below the style threshold`.

## Parte 5 — Benchmark y validación (Fase 5)

### Task 18b: Calibración de umbrales

**Files:**
- Create: `docs/qa/hito2-5/calibracion.md`, `core/crates/forma-bench/src/calibrate.rs`
- Modify: `forma_trace::QualityThresholds` y `forma_synth` (solo los valores, con `version += 1` si cambian)

**Interfaces:**
- Produces: `forma-bench calibrate --levels s,m,mhand,l,o --out <dir>`. Calcula para cada métrica de §4 y para cada componente de estilo:
  - la distribución en los glifos buenos (con verdad);
  - la distribución en los defectos inyectados (huecos cerrados, ranuras perdidas, terminales ajenos, grosor ±30 %);
  - la curva de falsos positivos y falsos negativos por umbral.

- [ ] **Step 1:** Test `calibration_flags_every_injected_defect`: con los umbrales elegidos, 0 defectos topológicos o de rasgo fino sin detectar.
- [ ] **Step 2:** Elegir los valores con el criterio de §8.5 de la especificación: entre los que no dejan pasar defectos, el que más conserva la irregularidad en O.
  - Documentar cada valor con su razón y ejemplos en `calibracion.md`.
  - **Si un valor relaja la fidelidad respecto a la especificación, consultar al responsable antes de cambiarlo.**
- [ ] **Step 3:** Commit: `feat(bench): threshold calibration; docs(qa): calibration report`.

### Task 19: Métricas completas del modo C y regresión en CI

**Files:**
- Modify: `forma-bench/src/metrics.rs` (todas las métricas de §8.3 sin agente, con **fidelidad geométrica y calidad tipográfica en secciones separadas**), `.github/workflows/ci.yml` (paso `forma-bench run --mode c --level s,l,o` + umbral; M y M-hand solo en `nightly.yml`), `tools/oracle/verify_hito2.py` o el nuevo `verify_hito2_5.py` (OTS, fontspector, HarfBuzz sobre las salidas del modo C)
- Test: `core/crates/forma-bench/tests/regression.rs`

**Interfaces:**
- Produces:
  - `results.json` por caso:
    - `iou_mask` y `iou_truth`, `hausdorff_p95_px`;
    - `topology_ok_pct`, `undetected_topology_errors`, `thin_kept_pct`;
    - `g1_violations`, `nodes_per_1000`;
    - `spacing_mae_pct_upm`, `kerning_corr`, `kerning_mae`, `collisions`;
    - `synth_created`, `synth_needs_design`, `synth_score_min`;
    - `validate_errors`, `ots_ok`, `fontspector_fail`, `fontspector_warn`;
    - `engine_ms` por etapa.
  - `irregularity_kept_pct` en O.
  - Puerta de CI (S, L y O): `irregularity_kept_pct ≥ 80` en O, `undetected_topology_errors == 0`, `thin_kept_pct == 100`, 0 uniones accidentales, `validate_errors == 0`, `ots_ok`, e IoU no menor que la línea base guardada en `core/crates/forma-bench/baseline.json`. Si el baseline cambia, el PR lo explica.

- [ ] **Step 1: Tests que fallan:** `regression_gate_on_s_and_l`, `m_metrics_against_truth` (marcado `#[ignore]` en CI rápida y ejecutado en nightly).
- [ ] **Step 2–3:** Implementar; tests → PASS en local en los 3 SO vía CI.
- [ ] **Step 4:** Commit: `ci: mode C quality regression gate on S and L; M in nightly`.

### Task 20: Benchmark con agentes A contra B (controlador, local)

**Files:**
- Create: `docs/qa/hito2-5/benchmark.md`, `docs/qa/hito2-5/README.md`

**Interfaces:**
- Consumes: Tasks 3 y 19.

- [ ] **Step 1:** Modo B, con presupuesto total ≤ US$ 15 y tope de turnos por corrida:
  - Claude Code: N=3 sobre S y N=1 sobre Inter 110 px;
  - Codex: N=1 sobre S.
  - Instrucción `mode_b.txt`: misma meta que `mode_a.txt`, sin pistas de herramientas.
  - Política `prompted` con aprobación humana simulada por el script mediante `plan_commit` desde un segundo cliente MCP del controlador, contabilizada como 1 intervención.
- [ ] **Step 2:** Modo C en todo el corpus (S, M, M-hand, L y O). Es determinista y barato.
- [ ] **Step 3:** Alfabeto privado (local, `~/fonts/hermana/`, nunca en el repo): modo B con Claude Code, N=1.
  - Verificar el criterio 2 de §8.4: la E con la ranura y topología correcta; la H agrupada sin intervención.
  - Publicar **solo** métricas agregadas, sin imágenes, y solo si el responsable lo autoriza.
- [ ] **Step 4:** `benchmark.md` con las tablas A contra B por cliente con todas las métricas de §8.3 (media ± desviación), la comparación con la línea base Alphabetical (986 s, 173 turnos, 6,94 USD) y el veredicto sobre las hipótesis "< 2 min" y "< 0,50 USD" (confirmada o refutada, con datos).
- [ ] **Step 5:** Commit: `docs(qa): milestone 2.5 benchmark, mode A vs B vs C`.

### Task 21: Documentación, criterios y cierre

**Files:**
- Modify: `docs/qa/hito2-5/decisiones.md`, `docs/04-roadmap.md`, `CLAUDE.md`, la especificación (sección de enmiendas si la implementación cambió algo), `docs/api/` (regenerada)

- [ ] **Step 1:** Comprobar uno por uno los 6 criterios de §8.4 con la evidencia (tests, `results.json` y `benchmark.md`) y anotarlos en `README.md`.
- [ ] **Step 2:** `cargo test --workspace --locked`, clippy, `deny`, cobertura ≥ 80 % en `forma-trace`, `forma-synth` y `forma-engine`, y oráculos → PASS.
- [ ] **Step 3:** Commit: `docs: milestone 2.5 status and acceptance evidence`. Presentar al responsable para decidir push y PR.
