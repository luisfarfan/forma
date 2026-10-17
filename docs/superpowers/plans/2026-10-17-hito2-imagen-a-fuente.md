# Hito 2 — De una imagen a una fuente completa — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A partir de la imagen de un alfabeto, FORMA (vía MCP o CLI) produce una fuente unicase completa con componentes, espaciado y kerning, validada y exportable a TTF/WOFF2, en macOS, Windows y Ubuntu.

**Architecture:**
- **Modelo:** gana componentes (en `Layer`) y kerning (en `Font`, con entidades por par y por grupo).
- **Crates nuevas:** `forma-trace` (imagen → contornos) y `forma-spacing` (espaciado, kerning y colisiones).
- **`forma-geometry`:** gana ajuste de curvas, booleanas, trazo de esqueleto y aplanado de componentes.
- **Herramientas:** 15 nuevas en el registro de `forma-engine`. MCP y CLI derivan de él, como en el Hito 1.

**Tech Stack:**
- Rust 1.99.1.
- Dependencias del Hito 1.
- Nuevas: `image` 0.25.10 (sin features por defecto, `png` + `jpeg`), `visioncortex` 0.9.3 y `linesweeper` 0.5.0. Todas `MIT OR Apache-2.0`.

**Spec:** `docs/superpowers/specs/2026-10-17-hito2-imagen-a-fuente-design.md` (autoridad), junto con `CLAUDE.md`, `docs/03-architecture.md` y `docs/07-mcp-ai-architecture.md`.

## Global Constraints

**Reglas heredadas**
- Rigen las del Hito 1 (`CLAUDE.md`):
  - licencia `MIT OR Apache-2.0`, edición 2024, toolchain 1.99.1;
  - `#![forbid(unsafe_code)]`;
  - sin `unwrap()`/`expect()` en código de librería;
  - errores con `thiserror`;
  - `cargo fmt`, `cargo clippy --workspace --all-targets --locked -- -D warnings` y `cargo deny check licenses` en verde;
  - código, identificadores, comentarios y commits en inglés; documentación en `docs/` en español.
- Coordenadas `f64` en unidades de fuente, con y hacia arriba. Los exteriores son antihorarios (área con signo > 0) y los interiores horarios.

**Entorno**
- En los shells de herramientas `cargo` no está en el `PATH`: anteponer `export PATH="$HOME/.cargo/bin:$PATH"`. Los comandos `cargo` se ejecutan desde `core/`.

**Dependencias nuevas** (exactas, en `[workspace.dependencies]`):
- `image = { version = "0.25.10", default-features = false, features = ["png", "jpeg"] }`
- `visioncortex = "0.9.3"`
- `linesweeper = "0.5.0"`

**Transform**
- Convención: `x' = xx·x + yx·y + dx`, `y' = xy·x + yy·y + dy`. Equivale a `kurbo::Affine::new([xx, xy, yx, yy, dx, dy])` y a norad `AffineTransform { x_scale: xx, xy_scale: xy, yx_scale: yx, y_scale: yy, x_offset: dx, y_offset: dy }`.
- Es válido si es finito y |xx·yy − xy·yx| ≥ 1e-6.

**Formato v2**
- `FORMAT_VERSION = 2`. Se leen v1 y v2, y se escribe siempre v2.
- Las versiones > 2 dan `UnsupportedVersion`.
- `Layer.components` y `Font.kerning` llevan `#[serde(default)]`, para que el JSON v1 se lea tal cual.

**Entidades nuevas**
- `EntityKey::KerningGroup(KerningGroupId)`, con forma de texto `kerning_group:<uuid>`.
- `EntityKey::KerningPair(MasterId, KernRef, KernRef)`, con forma de texto `kerning_pair:<master-uuid>:<ref>:<ref>`.
- `KernRef` en texto es `g.<uuid>` (glifo) o `k.<uuid>` (grupo). Todos los UUID van en minúsculas con guiones.

**Umbral masivo**
- Más de 20 entidades afectadas. Cuentan los glifos, los pares de kerning y los grupos de kerning.

**Las 50 herramientas, en orden de registro**
- Las 35 del Hito 1, en su orden.
- Después: `component_set`, `glyph_compose`, `glyph_decompose`, `glyph_stroke`, `remove_overlap`, `spacing_auto`, `kerning_get`, `kerning_set`, `kerning_auto`, `kerning_check`, `charset_check`, `image_inspect`, `image_trace`, `trace_compare`, `glyph_measure`.

**Conjuntos de herramientas**
- Hay 3 conjuntos nuevos: `components`, `kerning` y `trace` (`Toolset::ALL` pasa a 11).

| Conjunto | Herramientas |
|---|---|
| `components` | `component_set`, `glyph_compose`, `glyph_decompose` |
| `glyphs` | `glyph_stroke` |
| `geometry` | `remove_overlap`, `glyph_measure` |
| `spacing` | `spacing_auto` |
| `kerning` | `kerning_get`, `kerning_set`, `kerning_auto`, `kerning_check` |
| `quality` | `charset_check` |
| `trace` | `image_inspect`, `image_trace`, `trace_compare` |

**Efectos**

| Efecto | Herramientas |
|---|---|
| `READ` | `kerning_get`, `kerning_check`, `charset_check`, `image_inspect`, `trace_compare`, `glyph_measure` |
| `DESTRUCTIVE` (pueden borrar o reemplazar datos) | `component_set`, `glyph_stroke`, `kerning_set`, `kerning_auto`, `image_trace` |
| `WRITE` | `glyph_compose`, `glyph_decompose`, `remove_overlap`, `spacing_auto` |

- Las descripciones de las destructivas dicen "may require approval depending on the server policy".

**Salidas**
- Devuelven imagen + resumen, sin `structuredContent`: `image_inspect` y `trace_compare`.
- El resto devuelve JSON con `outputSchema`.

**Límites**
- Imagen ≤ 64 MiB (67 108 864 bytes) y ≤ 50 000 000 píxeles.
- ≤ 500 candidatos por imagen.
- ≤ 200 asignaciones por `image_trace`.
- Nodos de esqueleto: cuentan contra `MAX_NODES` (100 000) y ≤ 10 000 por llamada.
- `kerning_check` ≤ 250 000 pares por llamada.
- Si se superan: `LIMIT_EXCEEDED`.

**Parámetros fijados por la especificación**

*Trazado*
- ΔE por defecto 25, en CIE76 sobre Lab D65.
- Sobremuestreo ×4, bilineal.
- Ruido: área < 2 % de la mediana.
- Fusión de piezas: solapamiento horizontal ≥ 50 % del ancho del más estrecho.
- visioncortex: `PathSimplifyMode::Spline`, umbral de esquina 60°, longitud 4.0, 10 iteraciones, empalme 45°.
- Simplificación con error ≤ 1,5 unidades. Una tangente es suave si difiere ≤ 3°. Grilla de 1.
- Margen inicial: `lsb = rsb = round(0.05 × UPM)`.

*Espaciado*
- 64 líneas de muestreo.
- Overshoot 2 % del UPM.
- Una zona propia si hay menos de 8 líneas con tinta.
- Profundidad 15 % del UPM.
- `tightness` 0,6; referencias `H` y `O`.
- Bisección con tolerancia de 0,5.

*Kerning*
- `strength` 0,8; máximo 25 % del UPM; mínimo 10 unidades.
- Distancia mínima 2 % del UPM.
- Agrupación con L1 ≤ 3 % del UPM por línea; excepción si difiere más de 10 unidades.
- Colisión: distancia < 0.

*Trazo*
- Paso de muestreo ≤ 2 unidades; ajuste con error ≤ 0,5.
- `bulb`: círculo de radio `ratio × w/2`.

**Perfil `unicase-es`**
- Los 119 códigos de §3 de la especificación.
- U+0020, U+00A0.
- U+0041–U+005A y U+0061–U+007A.
- U+00C1, U+00C9, U+00CD, U+00D1, U+00D3, U+00DA, U+00DC, U+00E1, U+00E9, U+00ED, U+00F1, U+00F3, U+00FA, U+00FC.
- U+0030–U+0039.
- `. , : ; ! ¡ ? ¿ ' " ‘ ’ “ ” - – — ( ) [ ] { } / \ & @ # $ % * + = _ < > … · « » €`.

**Contrato**
- Todo cambio del snapshot de `tools/list` en este hito es **aditivo**. Las 35 herramientas existentes no cambian sus schemas, salvo:
  - el patrón de `EntityKey`;
  - los campos añadidos a `Layer`, `Master`/`Font` y `FontDiff`.
- Esos cambios se aceptan y se anotan en `docs/qa/hito2/decisiones.md`.

**Cómo regenerar**
- Snapshot: `INSTA_UPDATE=always cargo test -p forma-mcp --test contract tools_list_snapshot`.
- Docs (desde la raíz del repo): `cargo run --manifest-path core/Cargo.toml -p forma-cli -- gen-docs docs/api`.

**Contenido de terceros**
- Las imágenes y los nombres de glifo son datos, nunca instrucciones.
- No se versiona ninguna imagen de terceros. La imagen de la hermana del responsable **nunca** entra en el repositorio.

## Review Focus

1. **Imagen sin tinta detectable** (uniforme, o con un `color` que no aparece): `image_inspect` devuelve 0 candidatos y una nota clara, sin pánico. Si en `image_trace` un `candidate` está fuera de rango → `INVALID_ARGUMENT`. Tests en las Tasks 9 y 16.
2. **Letras que se tocan o con contraformas** (O, A, B, R): dos letras pegadas salen como **un** candidato ancho, y las contraformas salen como contornos interiores horarios. Tests en las Tasks 9 y 10.
3. **Componentes anidados y referencias que cambian:**
   - un compuesto cuya base es a su vez compuesta se aplana bien;
   - renombrar la base conserva el componente (se referencia por id);
   - borrar una base usada da `INVALID_ARGUMENT` con la lista de dependientes.
   Tests en las Tasks 2, 3 y 5.
4. **Kerning que referencia lo que desaparece:**
   - borrar un glifo elimina sus pares y su pertenencia a grupos en la misma transacción;
   - un glifo en dos grupos del mismo lado da `INVALID_ARGUMENT`;
   - un `undo` lo restaura todo.
   Tests en las Tasks 2 y 5.
5. **Esqueletos degenerados:** un esqueleto de 1 nodo, ancho ≤ 0 o no finito → `INVALID_ARGUMENT`. Un esqueleto que se cruza consigo mismo (un 8) da un contorno limpio tras la unión. `remove_overlap` sobre un glifo limpio no cambia nada (`tx: null`). Tests en las Tasks 4 y 14.

---

## Parte A — Modelo, geometría y persistencia

### Task 1: Documentos de dirección (ADR 0008, ADR 0009, roadmap, CLAUDE.md)

**Files:**
- Create: `docs/adr/0008-roadmap-motor-primero.md`, `docs/adr/0009-trazado-y-booleanas.md`
- Modify: `docs/04-roadmap.md`, `CLAUDE.md`, `docs/06-risks.md`

**Interfaces:**
- Produces: documentos de referencia que las demás tareas citan (sin código).

- [ ] **Step 1:** Escribir **ADR 0008** con el formato de los ADR existentes (contexto, decisión, alternativas, consecuencias):
  - motor primero e hitos guiados por fuentes demo;
  - la interfaz gráfica (spike Qt/Slint, editor y distribución) pasa al final;
  - sin fuentes comerciales, ni siquiera en local;
  - alternativas descartadas: B (por capacidades) y C (script aparte).
- [ ] **Step 2:** Escribir **ADR 0009** sobre `image`, `visioncortex` y `linesweeper`:
  - para qué sirve cada una;
  - licencia y mantenimiento de cada una (stars y último push: vtracer/visioncortex ~7,3 k stars, activas en 2026-10; linesweeper 0.5, de jneem, activa en 2026-01);
  - alternativas: potrace, descartada por GPL; Skia PathOps, plan B detrás del trait `PathOps`; iOverlay, descartada porque solo trabaja con polígonos.
- [ ] **Step 3:** Reescribir `docs/04-roadmap.md`:
  - **Estado:** revisión 4, del 2026-10-17. Hito 1 completado.
  - **Diagrama nuevo:**
    - Hito 2: imagen → fuente completa;
    - Hito 3: fuente original líquida + muestra de dominio público (anchors, `mark`/`mkmk`, alternativas, ligaduras, features);
    - Hito 4: reconstrucción de fuentes OFL premium (importación de UFO, designspace y `.glyphs`; comparación con el binario oficial);
    - Hito 5: multi-master y variables;
    - Hito 6: CFF/OTF y hinting;
    - Hito 7: Python y plugins;
    - Hito 8: spike de UI y editor;
    - Hito 9: distribución y v1.0.
  - **El Hito 2 lleva los 8 criterios de aceptación** copiados de §4 de la especificación.
  - **La definición de "terminado"** deja de exigir UI desde el Hito 3: ahora la exige desde el hito de la UI.
  - **Lo que se conserva:** el texto del Hito 1 se conserva como histórico.
- [ ] **Step 4:** Actualizar `CLAUDE.md`:
  - **Estado:**
    - Hito 1 completado (CI en verde en los 3 SO, fuzzing nocturno en verde);
    - Hito 2 aprobado y en curso (rama `hito2`);
    - la UI va al final (ADR 0008).
  - **Lista de crates:** sumar `forma-trace` y `forma-spacing`.
  - **Regla nueva:** "las imágenes de terceros nunca se versionan".
  - **Regla de fuentes comerciales:** dejarla explícita: tampoco se reconstruyen en local.
  - **Entorno:** borrar "Falta la primera ejecución del fuzzing".
- [ ] **Step 5:** En `docs/06-risks.md`, añadir riesgos nuevos con mitigación: linesweeper joven, fidelidad del trazado a baja resolución, y estética subjetiva de los glifos generados.
- [ ] **Step 6:** Commit: `docs: engine-first roadmap (ADR 0008) and tracing dependencies (ADR 0009)`.

### Task 2: Componentes y kerning en el modelo, y sus entidades en todo el workspace

**Files:**
- Create: `core/crates/forma-model/src/component.rs`, `core/crates/forma-model/src/kerning.rs`
- Modify: `core/crates/forma-model/src/{lib.rs,glyph.rs,font.rs,ids.rs}`
- Modify (ripple de los `match` exhaustivos, correctos y no solo para compilar):
  - `core/crates/forma-edit/src/{document.rs,diff.rs}`
  - `core/crates/forma-engine/src/{recovery.rs,errors.rs,stale.rs,history.rs,tools/steps.rs,tools/plans.rs}`
  - `core/crates/forma-store/src/journal.rs`
- Modify (sitios que construyen `Layer`/`Font` literalmente): `forma-compile/src/preflight.rs`, `forma-edit/src/apply/{glyphs,outlines}.rs` y los tests listados en el mapa de código: `store`, `interop`, `render`, `model`, `edit`, `validate`, `compile`.
- Test: `core/crates/forma-model/tests/model.rs`, `core/crates/forma-edit/tests/kerning_entities.rs`

**Interfaces:**
- Produces (en `forma_model`):
  - `pub struct Transform { pub xx: f64, pub xy: f64, pub yx: f64, pub yy: f64, pub dx: f64, pub dy: f64 }`.
    - Derives: `Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema`.
    - `Transform::IDENTITY`, `Default` = `IDENTITY`.
    - `fn determinant(&self) -> f64`, `fn is_valid(&self) -> bool`, `fn apply(&self, x: f64, y: f64) -> (f64, f64)`, `fn then(&self, outer: &Transform) -> Transform`.
  - `pub struct Component { pub base: GlyphId, #[serde(default)] pub transform: Transform }`.
  - `Layer { advance_width, paths, #[serde(default, skip_serializing_if = "Vec::is_empty")] pub components: Vec<Component> }`.
  - `uuid_id!(KerningGroupId)`.
  - `pub enum KernSide { First, Second }` (serde `"first"|"second"`).
  - `pub enum KernRef { Glyph(GlyphId), Group(KerningGroupId) }`: `Copy, Ord, Hash`, `Display` y `FromStr` como `g.<uuid>`/`k.<uuid>`, serde como esa cadena, `JsonSchema` con patrón.
  - `pub struct KerningGroup { pub id: KerningGroupId, pub name: String, pub side: KernSide, pub members: Vec<GlyphId> }`.
  - `pub struct KerningPair { pub first: KernRef, pub second: KernRef, pub value: f64 }`, la forma de serialización.
  - `pub struct Kerning { pub groups: BTreeMap<KerningGroupId, KerningGroup>, pub pairs: BTreeMap<MasterId, BTreeMap<(KernRef, KernRef), f64>> }`.
    - Serde: `{"groups": [...ordenados por id], "pairs": {"<master>": [KerningPair...ordenados por (first, second)]}}`.
    - Métodos:
      - `fn is_empty(&self) -> bool`;
      - `fn pair(&self, master: MasterId, first: KernRef, second: KernRef) -> Option<f64>`;
      - `fn group_of(&self, glyph: GlyphId, side: KernSide) -> Option<KerningGroupId>`;
      - `fn group_by_name(&self, side: KernSide, name: &str) -> Option<&KerningGroup>`;
      - `fn effective(&self, master: MasterId, left: GlyphId, right: GlyphId) -> f64`. Orden UFO: glifo-glifo, glifo-grupo, grupo-glifo, grupo-grupo; 0 si no hay.
  - `Font { …, #[serde(default, skip_serializing_if = "Kerning::is_empty")] pub kerning: Kerning }`.
  - `EntityKey::{KerningGroup(KerningGroupId), KerningPair(MasterId, KernRef, KernRef)}`, con las formas de texto de Global Constraints. El patrón del `JsonSchema` las incluye.
  - `pub fn Font::components_using(&self, base: GlyphId) -> Vec<GlyphId>`: los glifos que usan `base` en algún master, sin duplicados, en el orden de los glifos.
  - `ModelError` nuevos (`#[non_exhaustive]` se conserva):
    - `UnknownComponentBase { glyph: String, base: GlyphId }`
    - `ComponentMissingLayer { glyph: String, base: String, master: MasterId }`
    - `ComponentCycle { glyph: String }`
    - `InvalidTransform { glyph: String }`
    - `KerningUnknownMaster(MasterId)`
    - `KerningUnknownGlyph(GlyphId)`
    - `KerningUnknownGroup(KerningGroupId)`
    - `KerningGroupWrongSide { group: String }`
    - `GlyphInTwoGroups { glyph: String, side: KernSide }`
    - `InvalidGroupName(String)`
    - `DuplicateGroupName { side: KernSide, name: String }`
    - `InvalidKerningValue { value: f64 }`
    - `GroupKeyMismatch { key: KerningGroupId, id: KerningGroupId }`
  - `validate_invariants` los comprueba todos:
    - el nombre de grupo cumple `is_valid_glyph_name`;
    - el valor de kerning es finito y |v| ≤ 2 × UPM;
    - un valor 0 almacenado es inválido (0 = sin par).
- Produces (ripple):
  - `Document::revert`/`restore` copian o quitan los pares y grupos según `before`.
  - `diff::changed_entities` detecta cambios de pares y grupos.
  - `FontDiff` gana `kerning_changed: bool`; `GlyphDiff` gana `components_changed: bool`.
  - `BoundedDiff` (engine) refleja ambos.
  - `JournalRecord` gana `#[serde(default, skip_serializing_if = "Option::is_none")] kerning: Option<Kerning>`.
    - `record_of` lo llena si la transacción toca alguna clave de kerning.
    - `apply_records` reemplaza `font.kerning`.
  - Los detalles de conflicto nombran los pares como `<first>/<second>` con los nombres resueltos.
  - `Steps` cuenta en `affected` las claves `Glyph`, `KerningPair` y `KerningGroup`. El campo se renombra a `affected_entities` si hace falta; el umbral sigue siendo 20.

- [ ] **Step 1: Tests que fallan** (`forma-model/tests/model.rs`):
  - `transform_identity_and_determinant`: `IDENTITY.apply(3.0, 4.0) == (3.0, 4.0)`; `Transform{xx:2,..IDENTITY}.determinant() == 2.0`; una matriz singular da `!is_valid()`.
  - `component_roundtrip_and_v1_json`: un `Layer` de JSON v1 sin `components` deserializa con `components.is_empty()`. Un layer con componente hace round-trip, y el JSON omite `components` cuando está vacío.
  - `component_invariants`: una base inexistente da `UnknownComponentBase`; A→B→A da `ComponentCycle`; una transformación singular da `InvalidTransform`; una base sin capa en el master da `ComponentMissingLayer`.
  - `kerning_serde_is_canonical`: dos pares insertados en desorden serializan ordenados por `(first, second)`, y un `Font` sin kerning no emite la clave `kerning`.
  - `kerning_invariants`: un glifo en dos grupos `First` da `GlyphInTwoGroups`; un grupo `Second` usado como `first` da `KerningGroupWrongSide`; un valor 0 o `NaN` da `InvalidKerningValue`; un nombre de grupo `"1x"` da `InvalidGroupName`.
  - `effective_kerning_lookup_order`: con glifo-glifo = −50, glifo-grupo = −30 y grupo-grupo = −10, `effective` devuelve −50. Al quitar el glifo-glifo devuelve −30, y si no hay nada, 0.
  - `entity_key_text_forms`: round-trip `Display`/`FromStr` de `kerning_group:<uuid>` y de `kerning_pair:<m>:g.<u>:k.<u>`. Las formas no canónicas (mayúsculas, sin prefijo) dan error.
- [ ] **Step 2:** Run `cargo test -p forma-model --locked` → FAIL (los tipos no existen).
- [ ] **Step 3:** Implementar los tipos de Interfaces en `component.rs`, `kerning.rs`, `glyph.rs`, `font.rs` e `ids.rs`, y las invariantes en `validate_invariants`, después de las existentes.
- [ ] **Step 4:** Ripple. Implementar cada `match` exhaustivo nuevo con su semántica real y arreglar los literales de struct. Test `forma-edit/tests/kerning_entities.rs`, construyendo el kerning directamente en el `Font` del documento con un comando de prueba existente donde haga falta:
  - `revert_restores_kerning_pair`: un `Document` cuyo `restore(base_rev, Rev(0))` vuelve a un font sin un par añadido después → el par desaparece y `touched` contiene su `KerningPair`.
  - `diff_flags_kerning_and_components`.
- [ ] **Step 5:** Run `cargo test --workspace --locked` → PASS, y `cargo clippy --workspace --all-targets --locked -- -D warnings` limpio.
- [ ] **Step 6:** Commit: `feat(model): glyph components and kerning as first-class entities`.

### Task 3: Transformaciones, aplanado de componentes y ajuste de curvas (`forma-geometry`)

**Files:**
- Create: `core/crates/forma-geometry/src/{transform.rs,components.rs,fit.rs}`
- Modify: `core/crates/forma-geometry/src/lib.rs`
- Test: `core/crates/forma-geometry/tests/{components.rs,fit.rs}`

**Interfaces:**
- Consumes: `Transform`, `Component`, `Font`, `Layer` (Task 2).
- Produces:
  - `pub fn transform_paths(paths: &[Path], t: &Transform) -> Vec<Path>`. Si `det < 0`, invierte cada contorno para conservar la dirección. Los ids se conservan.
  - `pub fn flatten_layer(font: &Font, glyph: GlyphId, master: MasterId) -> Result<Vec<Path>, GeometryError>`. Devuelve los contornos propios más los de cada componente, transformados recursivamente. Renumera los ids desde 0. Da `GeometryError::InvalidPath` ante una base sin capa o un ciclo (defensa en profundidad).
  - `pub fn layer_bbox(font: &Font, glyph: GlyphId, master: MasterId) -> Result<Option<kurbo::Rect>, GeometryError>`.
  - `pub fn fit_cubics(points: &[kurbo::Point], max_error: f64, closed: bool) -> Vec<kurbo::CubicBez>`. Es el algoritmo de Schneider (Graphics Gems, 1990), con implementación propia: tangentes en los extremos, parametrización por longitud de cuerda, reparametrización Newton–Raphson hasta 4 veces, y división en el punto de error máximo. Para `closed`, la tangente en el punto de unión es continua.
  - `pub fn simplify_path(path: &Path, max_error: f64) -> Result<Path, GeometryError>`. Reajusta cada tramo entre esquinas, donde una esquina es un nodo no `smooth` o un ángulo de tangentes > 3°. Las esquinas se conservan.

- [ ] **Step 1: Tests que fallan:**
  - `flatten_nested_components`: `Aacute = A + acutecomb(dx=200, dy=750)` y `Aacute2 = Aacute(dx=10)` → el bbox de `flatten_layer(Aacute2)` es el de `A` ∪ acento desplazado en +10. Un espejo (`xx=-1`) mantiene la dirección (`signed_area > 0` en el exterior).
  - `fit_respects_max_error` (proptest, 1 000 casos): los puntos muestreados de una cúbica aleatoria con ruido ≤ 0,2 dan un ajuste cuya distancia máxima a los puntos es ≤ `max_error` (1,5).
  - `fit_circle_uses_few_segments`: 360 puntos de un círculo de radio 300 con `max_error = 0.5` dan entre 4 y 8 cúbicas.
  - `simplify_keeps_corners`: un cuadrado con un lado sobremuestreado (20 nodos colineales) se simplifica a 4 nodos `line`.
- [ ] **Step 2:** Run `cargo test -p forma-geometry --locked` → FAIL.
- [ ] **Step 3:** Implementar las funciones de Interfaces.
- [ ] **Step 4:** Run → PASS. Clippy limpio.
- [ ] **Step 5:** Commit: `feat(geometry): component flattening, transforms and cubic curve fitting`.

### Task 4: Unión de contornos (linesweeper) y trazo de esqueleto

**Files:**
- Create: `core/crates/forma-geometry/src/{boolean.rs,stroke.rs}`
- Modify: `core/Cargo.toml` (dependencia `linesweeper`), `core/crates/forma-geometry/{Cargo.toml,src/lib.rs}`
- Test: `core/crates/forma-geometry/tests/{boolean.rs,stroke.rs}`

**Interfaces:**
- Consumes: `fit_cubics`, `simplify_path`, `normalize_direction`, `fix_paths` (Task 3 y existentes).
- Produces:
  - `pub trait PathOps { fn union(&self, paths: &[Path]) -> Result<Vec<Path>, GeometryError>; }`.
  - `pub struct Linesweeper;` implementa `PathOps` con `linesweeper::binary_op(all, empty, FillRule::NonZero, BinaryOp::Union)`. El resultado se convierte con `from_bezpath`, se normaliza la dirección y se aplica `fix_paths(&[MissingExtrema, DuplicatePoints], 0.0)`.
  - `pub fn remove_overlap(paths: &[Path]) -> Result<Vec<Path>, GeometryError>`, que usa `Linesweeper`. Si linesweeper devuelve error → `GeometryError::Degenerate(..)`.
  - `pub struct SkeletonNode { pub x: f64, pub y: f64, pub kind: NodeKind, #[serde(default)] pub width: Option<f64> }`, con `Serialize`, `Deserialize` y `JsonSchema`.
  - `pub struct Skeleton { pub nodes: Vec<SkeletonNode>, #[serde(default)] pub closed: bool }`.
  - `pub enum Cap { Round, Bulb { ratio: f64 } }`, con serde `{"round": null}`/`{"bulb": {"ratio": 1.4}}` o la forma etiquetada que elija el implementador (documentada en el schema).
  - `pub struct StrokeStyle { pub width: f64, pub cap: Cap }`.
  - `pub fn stroke(skeletons: &[Skeleton], style: &StrokeStyle) -> Result<Vec<Path>, GeometryError>`. Algoritmo:
    1. muestrear cada esqueleto con paso ≤ 2 unidades;
    2. calcular el ancho por interpolación lineal entre los nodos on-curve (`width` del nodo o `style.width`);
    3. desplazar ±w/2 por la normal;
    4. reajustar con `fit_cubics(max_error = 0.5)`;
    5. tapas `Round`: semicírculo de diámetro w; `Bulb`: además, un círculo de radio `ratio × w/2` en cada extremo abierto;
    6. unir todo con `remove_overlap`.
    - Si un esqueleto tiene < 2 nodos on-curve, o un ancho ≤ 0 o no finito → `GeometryError::InvalidPath`.

- [ ] **Step 1: Tests que fallan:**
  - `union_of_two_circles_has_expected_area`: dos círculos de radio 100 con centros a 100 de distancia, cuya área de unión analítica es 2πr² − área de la lente; el resultado está a ±0,5 % y tiene 1 contorno exterior antihorario.
  - `union_keeps_counters`: una "O" (exterior + interior) más un rectángulo que la toca por fuera da un exterior y 1 interior horario.
  - `union_is_idempotent` (proptest, 200 casos de unión de rectángulos y círculos aleatorios): `remove_overlap(remove_overlap(x))` tiene el mismo número de contornos y área a ±0,1 % que `remove_overlap(x)`.
  - `no_self_intersections_after_union`: comprobar con el propio `linesweeper` (o con un test de cruce de segmentos aplanados) que el resultado no tiene cruces.
  - `stroke_segment_has_width`: un esqueleto recto de (0,0) a (0,500) con `width = 80` y `Round` da un ancho medido a media altura de 80 ± 1, y una altura total de 500 + 80 ± 1.
  - `stroke_bulb_terminals`: `Bulb{ratio:1.5}` da un ancho en el extremo de 120 ± 2.
  - `stroke_variable_width`: los nodos de ancho 60 → 120 dan anchos interpolados en t = 0,5 de 90 ± 2.
  - `stroke_figure_eight_is_clean`: un esqueleto cerrado que se cruza da un resultado unido, sin cruces (Review Focus 5).
  - `stroke_rejects_degenerate`: un solo nodo, `width = 0` o `width = NaN` → `Err`.
- [ ] **Step 2:** Run `cargo test -p forma-geometry --locked` → FAIL.
- [ ] **Step 3:** Agregar `linesweeper = "0.5.0"` a `[workspace.dependencies]` y a `forma-geometry`, y luego implementar.
- [ ] **Step 4:** Run tests → PASS; `cargo deny check licenses` → `licenses ok`. Clippy limpio.
- [ ] **Step 5:** Commit: `feat(geometry): overlap removal (linesweeper) and skeleton strokes`.

### Task 5: Comandos de componentes, booleanas y kerning (`forma-edit`)

**Files:**
- Create: `core/crates/forma-edit/src/apply/{components.rs,kerning.rs}`
- Modify:
  - `core/crates/forma-edit/src/{command.rs,apply.rs,lib.rs}` y `apply/{glyphs.rs,segments.rs}`, la parte de `SetMetrics` con componentes. Si `SetMetrics` vive en otro archivo, en ese.
  - `core/crates/forma-engine/src/tools/{steps.rs,limits.rs}`
- Test: `core/crates/forma-edit/tests/{components.rs,kerning.rs}`, `core/crates/forma-engine/tests/approvals.rs` (casos nuevos)

**Interfaces:**
- Consumes: Tasks 2 a 4.
- Produces, como variantes de `Command`, con la misma convención `tag = "op"` y snake_case:
  - `SetComponents { glyph: GlyphRef, #[serde(default)] master: Option<MasterId>, components: Vec<ComponentInput> }`, con `ComponentInput { base: GlyphRef, #[serde(default)] transform: Transform }`.
  - `Decompose { glyphs: Vec<GlyphRef>, #[serde(default)] master: Option<MasterId> }`. Aplana los componentes en contornos y deja `components` vacío.
  - `RemoveOverlap { glyphs: Vec<GlyphRef>, #[serde(default)] master: Option<MasterId> }`. Los glifos sin cambios no se tocan.
  - `SetKerningPair { #[serde(default)] master: Option<MasterId>, first: KernRefInput, second: KernRefInput, value: f64 }`, con `KernRefInput` externamente etiquetado: `{"glyph": GlyphRef}` | `{"group": String}` (nombre del grupo en el lado que corresponde). Un valor 0 quita el par.
  - `SetKerningGroup { name: String, side: KernSide, members: Vec<GlyphRef> }`. Crea, o reemplaza los miembros del grupo `(side, name)`.
  - `DeleteKerningGroup { name: String, side: KernSide }`. También quita los pares que lo referencian.

**Semántica:**
- **`DeleteGlyphs`:**
  - Si algún glifo borrado es base de un componente de un glifo **no** borrado en el mismo comando → `EditError::InvalidArgument("glyph '<name>' is used as a component by: <names>")`.
  - Si no, quita el glifo de sus grupos y elimina los pares que lo referencian. Toca sus claves.
- **`SetMetrics` con `lsb`:** desplaza también el `dx` de los componentes. El bbox para `lsb`/`rsb` usa `layer_bbox` (componentes incluidos).
- **Claves tocadas:**
  - `SetComponents`, `Decompose` y `RemoveOverlap` → `Glyph(id)`.
  - `SetKerningPair` → `KerningPair(master, ref, ref)`.
  - `SetKerningGroup` → `KerningGroup(id)`.
  - `DeleteKerningGroup` → `KerningGroup(id)` más cada `KerningPair` quitado.
- **`affected_glyph_count`:** cuenta los glifos. Para kerning, 0.
- **`Steps::destroys`:**
  - `SetComponents` que reemplaza una lista **no vacía** de componentes en ese master;
  - `SetKerningPair` con `value == 0` sobre un par existente;
  - `DeleteKerningGroup`.
- **`command_nodes`:** las nuevas cuentan 0, salvo `RemoveOverlap`, que cuenta los nodos actuales del glifo.

- [ ] **Step 1: Tests que fallan** (`components.rs`):
  - `set_components_and_render_bbox`.
  - `decompose_preserves_outline`: el área y el bbox de `flatten_layer` antes de descomponer son iguales a los de `paths` después.
  - `delete_used_base_is_refused` (Review Focus 3).
  - `delete_base_and_users_together_is_allowed`.
  - `rename_base_keeps_components`.
  - `lsb_moves_components`.
  - `remove_overlap_command_unions_and_noop_on_clean`: el glifo limpio no aparece en `touched`.
- [ ] **Step 2: Tests que fallan** (`kerning.rs`):
  - `set_pair_and_remove_with_zero`.
  - `group_create_replace_members`.
  - `glyph_in_two_groups_same_side_is_refused` (Review Focus 4).
  - `delete_glyph_cascades_kerning_and_undo_restores`: borrar `A`, que tiene pares y pertenece a un grupo, quita los pares y la pertenencia; `undo` lo restaura todo (Review Focus 4).
  - `pairs_by_two_actors_do_not_conflict`: el actor `user` fija A/V y `agent:x` fija T/o con un `base_rev` anterior → ambos aplican.
  - `same_pair_conflicts`: el mismo par con `base_rev` viejo → `Conflict`.
  - `property_n_kerning_commands_then_n_undos_restore` (proptest, 1 000 casos).
- [ ] **Step 3: Tests que fallan** (`forma-engine/tests/approvals.rs`):
  - `removing_a_pair_is_destructive`.
  - `twenty_one_pairs_is_mass`: `edit_batch` con 21 `kerning_set`… Si `kerning_set` aún no existe, usar `Steps::of` directamente con 21 `SetKerningPair` y comprobar `affected > 20`.
- [ ] **Step 4:** Run `cargo test -p forma-edit -p forma-engine --locked` → FAIL.
- [ ] **Step 5:** Implementar.
- [ ] **Step 6:** Run `cargo test --workspace --locked` → PASS. Clippy limpio.
- [ ] **Step 7:** Commit: `feat(edit): component, overlap and kerning commands with cascading deletes`.

### Task 6: Formato v2 y migración desde v1 (`forma-store`)

**Files:**
- Modify: `core/crates/forma-store/src/{parts.rs,sqlite.rs,package.rs,lib.rs}`
- Modify: `core/crates/forma-store/tests/roundtrip.rs` (versiones 1 → 2 y "más nueva" 2 → 3), `core/crates/forma-store/tests/props.rs`
- Create: `core/crates/forma-store/tests/migration.rs`, `core/crates/forma-store/tests/fixtures/v1/{doc.forma,doc.formapackage/…}`. Los fixtures v1 se generan **con el código actual antes de cambiar la versión** (paso 1) y se versionan.
- Create: `core/fuzz/seeds/load_forma/valid_v2.seed`, `core/fuzz/seeds/load_package/manifest_v2.seed`

**Interfaces:**
- Consumes: el modelo de la Task 2.
- Produces:
  - `FORMAT_VERSION = 2`.
  - `pub const READABLE_VERSIONS: [u32; 2] = [1, 2]`.
  - `fn migrate_font_document(version: u32, doc: serde_json::Value) -> Result<serde_json::Value, StoreError>` y `fn migrate_glyph_document(version: u32, doc: Value) -> Result<Value, StoreError>`: puras y sin E/S. v1 → v2 no cambia nada, porque los defaults de serde cubren los campos, pero existen como punto de extensión y se testean.
  - `ensure_finite` cubre las transformaciones de los componentes y los valores de kerning.
- **Carga:**
  - SQLite acepta `user_version` 1 y 2 (y `format_version` igual); el resto → `UnsupportedVersion`/`Invalid`, como hoy.
  - Package acepta `format_version` 1 y 2.
  - **Guardar siempre escribe v2**, incluso sobre un archivo v1. Eso no es destructivo: el contenido es el mismo.

- [ ] **Step 1:** Antes de tocar el código, generar los fixtures v1 con un test `#[ignore]` (`generate_v1_fixtures`) que guarda un documento de muestra (3 glifos) con el código actual. Ejecutarlo y versionar la salida.
- [ ] **Step 2: Tests que fallan** (`migration.rs`):
  - `v1_forma_loads_and_saves_as_v2`: `load` del fixture v1 → `save` → `user_version == 2`, y el contenido es igual al del fixture.
  - `v1_package_loads_and_saves_as_v2`.
  - `v3_is_rejected_with_unsupported_version`.
  - `migrate_documents_are_pure`: misma entrada, misma salida, y v2 → v2 es la identidad.
  - `kerning_and_components_roundtrip_both_formats`: la fuente con grupos, pares y compuestos da `load(save(x)) == x` en `.forma` y `.formapackage`.
  - `kerning_in_font_json_is_one_entry_per_pair`: el `font.json` canónico tiene los pares ordenados.
- [ ] **Step 3:** Ampliar `props.rs`: el generador de fuentes incluye componentes válidos y kerning, y la propiedad `load(save(doc)) == doc` corre 1 000 casos en ambos formatos.
- [ ] **Step 4:** Run `cargo test -p forma-store --locked` → FAIL, y luego implementar → PASS.
- [ ] **Step 5:** Crear las semillas de fuzz v2, guardando una fuente con kerning y componentes.
- [ ] **Step 6:** Run `cargo test --workspace --locked` → PASS.
- [ ] **Step 7:** Commit: `feat(store): format v2 with components and kerning, migrating v1 documents`.

### Task 7: Render de componentes, kerning en texto y máscara de cobertura (`forma-render`)

**Files:**
- Create: `core/crates/forma-render/src/mask.rs`
- Modify: `core/crates/forma-render/src/{lib.rs,draw.rs,text.rs,svg.rs,compare.rs}`
- Test: `core/crates/forma-render/tests/render.rs`

**Interfaces:**
- Consumes: `flatten_layer` (Task 3), `Kerning::effective` (Task 2).
- Produces:
  - Todas las vistas usan `flatten_layer` (los componentes se ven).
  - `text_png` suma `font.kerning.effective(default_master, prev, next)` entre glifos consecutivos.
  - `pub struct Mask { pub width: u32, pub height: u32, pub data: Vec<u8> }`, con la cobertura de 0 a 255 por píxel, en filas de arriba abajo.
  - `pub fn coverage_mask(paths: &[Path], to_pixels: &Transform, width: u32, height: u32) -> Result<Mask, RenderError>`. Usa relleno no-cero y el mismo vello_cpu determinista. `to_pixels` mapea unidades de fuente a píxeles (con y hacia abajo), y la cobertura sale del canal alfa o de la luminancia invertida.
  - `pub fn mask_iou(a: &Mask, b: &Mask, threshold: u8) -> Result<f64, RenderError>`. Las dimensiones deben coincidir. Un píxel está "dentro" si cobertura ≥ threshold (128). IoU = |A∩B| / |A∪B|, y 1.0 si ambas están vacías.
  - Límite: `width × height ≤ 50 000 000` → `RenderError::InvalidSize`.

- [ ] **Step 1: Tests que fallan:**
  - `composite_glyph_renders_like_decomposed`: el PNG de `Aacute` (componentes) es igual píxel a píxel al de su versión descompuesta.
  - `text_applies_kerning`: "AV" con A/V = −100 da un ancho total igual a `adv(A) + adv(V) − 100` (en píxeles a la escala del texto, ±1).
  - `coverage_mask_of_square`: un cuadrado de 100×100 unidades a 1 px/unidad da 10 000 ± 200 píxeles ≥ 128.
  - `iou_identical_is_one_and_disjoint_is_zero`.
- [ ] **Step 2:** Run `cargo test -p forma-render --locked` → FAIL, y luego implementar → PASS.
- [ ] **Step 3:** Commit: `feat(render): composite glyphs, kerned text previews and coverage masks`.

### Task 8: UFO, compilación y validación con componentes y kerning; perfiles de juego de caracteres

**Files:**
- Create: `core/crates/forma-validate/src/charset.rs`
- Modify:
  - `core/crates/forma-interop/src/ufo.rs`
  - `core/crates/forma-compile/src/preflight.rs`
  - `core/crates/forma-validate/src/{lib.rs,model_checks.rs,binary_checks.rs}`
- Test: `core/crates/forma-interop/tests/ufo.rs`, `core/crates/forma-compile/tests/*.rs` (nuevo `composites_and_kerning.rs`), `core/crates/forma-validate/tests/*.rs` (nuevo `charset.rs`)

**Interfaces:**
- Produces:
  - **`write_ufo`:**
    - escribe `norad::Component` por componente, con la convención de Transform de Global Constraints;
    - si la base no se exporta (`export == false`), ese componente se **descompone** en contornos;
    - escribe `ufo.groups`, con nombres `public.kern1.<name>`/`public.kern2.<name>`, y `ufo.kerning` del master exportado;
    - los pares que apuntan a glifos no exportados se omiten.
  - **`preflight`:** aplana los compuestos cuya base no está en la capa del master por defecto (no debería pasar, por las invariantes) y verifica que las coordenadas aplanadas caben en i16.
  - **`forma_validate`:**
    - check `glyph.mixed_contours_components` (Warning);
    - check `kerning.unexported_reference` (Warning);
    - check `binary.gpos` (Error si `!font.kerning.is_empty()` y el binario no tiene GPOS).
  - **`pub mod charset`:**
    - `pub fn profile(name: &str) -> Option<&'static [u32]>`, con `"unicase-es"` = los 119 códigos de Global Constraints;
    - `pub struct CharsetReport { pub required: usize, pub covered: usize, pub missing: Vec<u32>, pub composite: Vec<String>, pub outlined: Vec<String> }`;
    - `pub fn check_charset(font: &Font, codepoints: &[u32]) -> CharsetReport`. `composite` son los glifos que cubren algún código requerido y solo tienen componentes; `outlined`, los que tienen contornos.

- [ ] **Step 1: Tests que fallan:**
  - `ufo_writes_components_and_kerning`: norad lee el UFO y encuentra un componente con el transform esperado, el grupo `public.kern1.O`, y `kerning["public.kern1.O"]["V"] == -40`.
  - `unexported_base_is_decomposed_in_ufo`.
  - `compiled_font_has_composite_glyphs_and_gpos`: el binario tiene la tabla GPOS, y `glyf` de `Aacute` es compuesto (read-fonts: `Glyph::Composite`).
  - `validate_warns_on_mixed_glyph`.
  - `validate_errors_when_gpos_missing`: este caso exige un binario sintético; como alternativa, testear la función de check directamente.
  - `charset_profile_has_119_codepoints`.
  - `charset_reports_missing_and_composites`.
- [ ] **Step 2:** Run → FAIL, implementar → PASS (`cargo test -p forma-interop -p forma-compile -p forma-validate --locked`).
- [ ] **Step 3:** Run `cargo test --workspace --locked` → PASS.
- [ ] **Step 4:** Commit: `feat(interop,compile,validate): components, kerning and charset profiles end to end`.

---

## Parte B — Trazado y espaciado

### Task 9: `forma-trace` I — decodificar, paleta, máscara y candidatos

**Files:**
- Create: `core/crates/forma-trace/{Cargo.toml,src/{lib.rs,decode.rs,palette.rs,mask.rs,candidates.rs,annotate.rs,error.rs}}`
- Modify: `core/Cargo.toml` (`image`)
- Test: `core/crates/forma-trace/tests/analyze.rs`

**Interfaces:**
- Produces (crate `forma-trace`, dependencias `forma-model`, `forma-geometry`, `forma-render`, `image`, `thiserror`, `serde`, `schemars`):
  - `pub enum TraceError` (`#[non_exhaustive]`):
    - `Decode(String)`
    - `TooLarge { bytes: u64, pixels: u64 }`
    - `TooManyCandidates(usize)`
    - `CandidateOutOfRange { index: usize, count: usize }`
    - `Vectorize(String)`
    - `Render(String)`
    - `Geometry(String)`
  - `pub const MAX_IMAGE_BYTES: u64 = 67_108_864; pub const MAX_IMAGE_PIXELS: u64 = 50_000_000; pub const MAX_CANDIDATES: usize = 500;`
  - `pub struct TraceOptions { pub color: Option<[u8; 3]>, pub tolerance: f64, pub upsample: u32 }`. `Default` = `{ None, 25.0, 4 }`.
  - `pub fn decode(bytes: &[u8]) -> Result<image::RgbImage, TraceError>`:
    - comprueba los límites **antes** de decodificar los píxeles, leyendo las dimensiones con `image::ImageReader::with_format(...).into_dimensions()` (o equivalente);
    - si se superan → `TooLarge`;
    - solo acepta PNG y JPEG.
  - `pub struct PaletteEntry { pub color: [u8; 3], pub share: f64 }`.
  - `pub struct PxRect { pub x0: u32, pub y0: u32, pub x1: u32, pub y1: u32 }`, con límites exclusivos en `x1`/`y1`.
  - `pub struct Row { pub index: usize, pub top: f64, pub cap_px: f64, pub baseline_px: f64, pub bottom: f64 }`.
  - `pub struct Candidate { pub index: usize, pub row: usize, pub bbox: PxRect, pub area: u64 }`.
  - `pub struct Analysis { pub width: u32, pub height: u32, pub background: [u8; 3], pub ink: [u8; 3], pub palette: Vec<PaletteEntry>, pub rows: Vec<Row>, pub candidates: Vec<Candidate>, pub note: Option<String> }`. Todos con `Serialize`, `Deserialize` y `JsonSchema`.
  - `pub fn analyze(image: &image::RgbImage, opts: &TraceOptions) -> Result<Analysis, TraceError>`:
    1. el fondo es el color más frecuente del borde de 1 px, cuantizado a 5 bits por canal;
    2. la paleta sale de k-means (k ≤ 8, 10 iteraciones, semilla fija, sobre ≤ 50 000 píxeles muestreados en rejilla);
    3. la tinta es `opts.color` o el grupo no-fondo con más `share`;
    4. máscara suave → sobremuestreo ×`upsample` → binaria al 50 %;
    5. componentes conexos (conectividad 8) en la máscara sobremuestreada, con coordenadas reportadas en píxeles **originales**;
    6. ruido: área < 2 % de la mediana;
    7. filas por solapamiento vertical, ordenadas arriba → abajo y, dentro de cada fila, izquierda → derecha;
    8. fusión de piezas con solapamiento horizontal ≥ 50 % del más estrecho;
    9. métricas por fila: medianas de los bordes inferior y superior.
    - Determinista. Si no hay tinta: 0 candidatos y `note = Some("no ink found: …")`.
  - `pub(crate) fn binary_mask(image, analysis-or-opts) -> Mask` (`forma_render::Mask`), reutilizado por la Task 10.
  - `pub fn annotated_png(image: &image::RgbImage, analysis: &Analysis) -> Result<Vec<u8>, TraceError>`. Pinta la caja y el número de cada candidato con dígitos de mapa de bits propios (7×5), sin fuentes del sistema, y las líneas base y de mayúsculas de cada fila.

- [ ] **Step 1: Tests que fallan.** Las imágenes sintéticas se generan en el test con `image`: fondo negro, rectángulos y anillos naranja `#FF5A1F` y un círculo azul `#2A5BFF`.
  - `decode_rejects_oversized_dimensions`: una cabecera PNG de 10 000 × 10 000 → `TooLarge`, sin reservar 100 MP. Medirlo con un PNG mínimo cuyo IHDR declara ese tamaño.
  - `decode_rejects_garbage`: los bytes aleatorios dan `Decode`.
  - `palette_finds_background_and_ink`: la tinta queda a ±12 por canal de `#FF5A1F`, y el fondo es negro.
  - `blue_distractor_is_ignored`: 3 rectángulos naranja + 1 círculo azul → 3 candidatos.
  - `rows_and_order`: 2 filas de 3 formas → índices 0–2 en la fila 0 y 3–5 en la fila 1, de izquierda a derecha. Las líneas base son correctas ±1 px.
  - `pieces_merge_into_one_candidate`: una barra con un punto encima (una "i") → 1 candidato.
  - `touching_shapes_are_one_candidate`: dos cuadrados pegados → 1 candidato ancho (Review Focus 2).
  - `no_ink_gives_note`: una imagen uniforme → 0 candidatos y `note.is_some()` (Review Focus 1).
  - `analysis_is_deterministic`.
- [ ] **Step 2:** Run `cargo test -p forma-trace --locked` → FAIL, y luego implementar → PASS. `cargo deny check licenses` → ok.
- [ ] **Step 3:** Commit: `feat(trace): image decoding with limits, palette, ink mask and glyph candidates`.

### Task 10: `forma-trace` II — vectorizar, limpiar, fidelidad y medición de estilo

**Files:**
- Create: `core/crates/forma-trace/src/{vectorize.rs,cleanup.rs,fidelity.rs,measure.rs}`
- Modify: `core/Cargo.toml` (`visioncortex`), `core/crates/forma-trace/{Cargo.toml,src/lib.rs}`
- Test: `core/crates/forma-trace/tests/{vectorize.rs,measure.rs}`

**Interfaces:**
- Consumes: `analyze`, `binary_mask` (Task 9); `simplify_path`, `fix_paths`, `normalize_direction` (Task 3 y existentes); `coverage_mask`, `mask_iou` (Task 7).
- Produces:
  - `pub struct Placement { pub scale: f64, pub origin_px: (f64, f64), pub lsb: f64 }`. Escala en unidades por píxel original; `origin_px` = (`bbox.x0` del candidato, `baseline_px` de su fila).
  - `pub struct TracedGlyph { pub candidate: usize, pub paths: Vec<Path>, pub ink_width: f64, pub advance_width: f64, pub iou: f64, pub placement: Placement }`.
    - `advance_width = ink_width + 2 × lsb`, con `lsb = round(0.05 × units_per_em)`.
  - `pub fn trace(image: &RgbImage, opts: &TraceOptions, indices: &[usize], cap_height_units: f64, units_per_em: u16) -> Result<Vec<TracedGlyph>, TraceError>`:
    - escala = `cap_height_units / (baseline_px − cap_px)` de la fila del candidato;
    - recorte de la máscara binaria con 2 px de margen (en el espacio sobremuestreado);
    - visioncortex `BinaryImage` → `to_clusters(false)` → `to_compound_path(PathSimplifyMode::Spline, 60°, 4.0, 10, 45°)` en radianes donde la API lo pida;
    - de spline a `Path`: unidades de fuente, y arriba; `x = (px − bbox.x0)·scale + lsb`, `y = (baseline_px − py)·scale`;
    - limpieza: `simplify_path(1.5)` → `normalize_direction` → `fix_paths(&[DuplicatePoints, TinySegment, MissingExtrema, OffGrid, WrongDirection], 1.0)`, más el marcado `smooth` cuando las tangentes difieren ≤ 3°;
    - IoU según `fidelity`.
    - Un índice inválido → `CandidateOutOfRange`.
  - `pub struct Comparison { pub iou: f64, pub overlay_png: Vec<u8> }`.
  - `pub fn compare(image: &RgbImage, opts: &TraceOptions, candidate: usize, paths: &[Path], cap_height_units: f64) -> Result<Comparison, TraceError>`:
    - rasteriza `paths` con `coverage_mask` a la escala del candidato, alineando el **borde izquierdo del bbox de tinta** con `bbox.x0` y y = 0 con `baseline_px`;
    - calcula el IoU contra la máscara binaria del candidato, en píxeles originales;
    - superposición PNG: máscara en gris `#9A9A9A`, contorno en `#FF5A1F` y diferencias en `#E5484D`.
  - `pub struct StyleMeasure { pub stroke_median: f64, pub stroke_p10: f64, pub stroke_p90: f64, pub terminal_radius: Option<f64>, pub join_min: Option<f64> }`, en unidades de fuente.
  - `pub fn measure(font: &Font, glyph: GlyphId, master: MasterId) -> Result<StyleMeasure, TraceError>`:
    - rasteriza a 4 px por unidad × (1000 / UPM), con el lado mayor ≤ 2048 px;
    - transformada de distancia euclídea exacta (Felzenszwalb–Huttenlocher, implementación propia);
    - esqueleto por máximos locales;
    - grosor = 2 × distancia en el esqueleto (mediana, p10, p90);
    - `terminal_radius` = la distancia máxima en los extremos del esqueleto (los nodos de grado 1);
    - `join_min` = la distancia mínima en los nodos de grado ≥ 3.
    - `None` cuando no hay extremos o uniones.

- [ ] **Step 1: Tests que fallan:**
  - `traced_ring_has_counter`: un anillo sintético → 2 contornos, el exterior antihorario y el interior horario (Review Focus 2), con IoU ≥ 0,99.
  - `traced_rectangle_is_four_lines`: un rectángulo de 120 × 300 px → 1 contorno de 4 nodos `line` (±0 tras limpiar), con IoU ≥ 0,99.
  - `traced_circle_iou`: un círculo de radio 60 px → IoU ≥ 0,99 y ≤ 12 nodos on-curve.
  - `geometry_is_clean_after_trace`: `check_paths(paths, 1.0)` vacío.
  - `scale_follows_cap_height`: un candidato de 100 px de alto en una fila con `cap_px` 100 px y `cap_height_units = 700` → bbox de altura 700 ± 7.
  - `out_of_range_candidate_is_an_error`.
  - `measure_stroke_of_bar`: un glifo rectangular de 80 unidades de ancho → `stroke_median` 80 ± 3.
  - `measure_bulb_terminal`: un `stroke` con `Bulb{ratio:1.5}` y `w = 80` → `terminal_radius` 60 ± 4.
- [ ] **Step 2:** Run `cargo test -p forma-trace --locked` → FAIL, y luego implementar → PASS. `cargo deny check licenses` → ok.
- [ ] **Step 3:** Commit: `feat(trace): vectorization with cleanup, fidelity measurement and style measures`.

### Task 11: Imagen de prueba propia y fidelidad de extremo a extremo (criterio 1)

**Files:**
- Create: `tests/fixtures/hito2/{README.md,truth.json,specimen.png}`
- Create: `core/crates/forma-trace/tests/{specimen.rs,common/mod.rs}`

**Interfaces:**
- Consumes: Tasks 7, 9 y 10.
- Produces:
  - **`truth.json`:** contornos verdaderos en formato de glifo del Hito 1 (`name`, `unicodes`, `advance_width`, `svg_path`).
    - Incluye `A B C H O`, copiados de `tests/fixtures/hito1/glyphs.json`.
    - Incluye `F M R`, copiados de `docs/assets/forma-wordmark.forma` con `forma call glyph_get`.
    - UPM 1000, cap height 700.
    - Todo es material propio (README de licencia: MIT OR Apache-2.0, como el repo).
  - **`specimen.png`:** 1600 × 700 px, fondo `#111111`, tinta `#FF5A1F`.
    - Fila 0: `A B C H`. Fila 1: `O F M R`. Escala 0,16 px/unidad (cap = 112 px, como la imagen real).
    - Un círculo azul `#2A5BFF` de radio 60 px en la esquina inferior derecha.
    - Generado por `fn render_specimen(truth) -> RgbImage` en `tests/common/mod.rs`, con `coverage_mask` y mezcla lineal. El PNG se codifica con `image` (determinista).
  - `pub fn truth_font() -> Font` en `tests/common/mod.rs`.

- [ ] **Step 1: Tests:**
  - `specimen_is_up_to_date`: `render_specimen()` codificado es byte a byte igual al PNG versionado. Con `FORMA_REGENERATE_FIXTURES=1` lo reescribe.
  - `specimen_candidates_are_eight_in_two_rows`.
  - `specimen_traces_with_iou_above_097` (criterio 1, máscara): `trace` de los 8 candidatos → cada `iou ≥ 0.97`.
  - `specimen_traces_match_truth_outlines` (criterio 1, contorno): para cada letra, IoU por área en unidades de fuente entre el contorno trazado y el verdadero ≥ 0,97. Se calcula rasterizando ambos con `coverage_mask` a 1 px/unidad, alineados por el borde izquierdo del bbox y la línea base.
  - `specimen_glyphs_are_clean`: `check_paths` vacío para los 8.
- [ ] **Step 2:** Generar `specimen.png` con `FORMA_REGENERATE_FIXTURES=1 cargo test -p forma-trace --test specimen`, y luego `cargo test -p forma-trace --locked` → PASS sin la variable.
- [ ] **Step 3:** Commit: `test(trace): own specimen image with ground-truth outlines (criterion 1)`.

### Task 12: `forma-spacing` I — perfiles, espaciado automático y colisiones

**Files:**
- Create: `core/crates/forma-spacing/{Cargo.toml,src/{lib.rs,profile.rs,spacing.rs,collision.rs,error.rs}}`
- Test: `core/crates/forma-spacing/tests/{spacing.rs,collision.rs}`

**Interfaces:**
- Consumes: `flatten_layer`, `layer_bbox` (Task 3), `Kerning::effective` (Task 2).
- Produces (crate `forma-spacing`, dependencias `forma-model`, `forma-geometry`, `kurbo`, `thiserror`, `serde`, `schemars`):
  - `pub enum SpacingError` (`#[non_exhaustive]`): `Geometry(String)`, `MissingReference(String)`, `MissingLayer(String)`, `InvalidParameter(String)`.
  - `pub struct Zone { pub bottom: f64, pub top: f64 }`.
  - `pub fn measurement_zone(font: &Font, master: MasterId) -> Zone`, que va de `−0.02·UPM` a `cap_height + 0.02·UPM`.
  - `pub struct Profile { pub ys: Vec<f64>, pub left: Vec<Option<f64>>, pub right: Vec<Option<f64>> }`.
  - `pub fn profile(font: &Font, glyph: GlyphId, master: MasterId, zone: Zone, lines: usize) -> Result<Profile, SpacingError>`:
    - intersecta cada línea horizontal con el contorno aplanado y toma la x mínima y máxima, con relleno no-cero;
    - si hay < 8 líneas con tinta, recalcula sobre la altura de tinta propia.
  - `pub struct SpacingParams { pub tightness: f64, pub references: [String; 2], pub depth_ratio: f64, pub lines: usize }`. `Default` = `{ 0.6, ["H", "O"], 0.15, 64 }`.
  - `pub struct Sidebearings { pub glyph: GlyphId, pub lsb: f64, pub rsb: f64 }`.
  - `pub fn auto_spacing(font: &Font, master: MasterId, glyphs: &[GlyphId], params: &SpacingParams) -> Result<Vec<Sidebearings>, SpacingError>`, con el algoritmo de §6.2 de la especificación:
    - el ancho de trazo de `H` es la mediana del ancho de tinta horizontal de los tramos en la mitad de la altura de mayúsculas, o el ancho del primer tramo;
    - margen de `H` = `tightness × ancho`;
    - el blanco objetivo se calcula sobre `H` con ese margen;
    - bisección por lado (tolerancia 0,5);
    - `O` se calcula, pero no fija el objetivo: el espaciado de `O` sale del mismo blanco;
    - **excluye** los glifos que solo tienen componentes y los glifos sin tinta;
    - los resultados se redondean a enteros.
  - `pub struct Collision { pub left: GlyphId, pub right: GlyphId, pub distance: f64 }`.
  - `pub fn collisions(font: &Font, master: MasterId, pairs: &[(GlyphId, GlyphId)], min_distance: f64) -> Result<Vec<Collision>, SpacingError>`:
    - para cada par, con el avance del izquierdo más el kerning efectivo, calcula la distancia horizontal mínima entre tinta en 256 líneas sobre la unión de alturas de tinta;
    - si `distance < min_distance`, es colisión;
    - ordenadas por distancia ascendente.
  - `pub const MAX_CHECK_PAIRS: usize = 250_000`.

- [ ] **Step 1: Tests que fallan:**
  - `symmetric_glyph_gets_symmetric_sidebearings` (criterio 5): para `H`, `O` y `A` del Hito 1, |lsb − rsb| ≤ 1.
  - `spacing_is_idempotent`: aplicar los márgenes y recalcular cambia ≤ 1 unidad.
  - `composites_are_skipped`.
  - `tightness_changes_spacing`: con tightness 0,3, `H` sale más apretada que con 0,6.
  - `missing_reference_is_an_error`.
  - `collision_detects_overlap`: "A" "V" con kerning de −400 → colisión con distancia < 0.
  - `no_collision_for_spaced_pair`.
- [ ] **Step 2:** Run `cargo test -p forma-spacing --locked` → FAIL, y luego implementar → PASS.
- [ ] **Step 3:** Commit: `feat(spacing): glyph profiles, automatic spacing and collision checks`.

### Task 13: `forma-spacing` II — kerning automático con grupos

**Files:**
- Create: `core/crates/forma-spacing/src/kerning.rs`
- Test: `core/crates/forma-spacing/tests/kerning.rs`

**Interfaces:**
- Consumes: `profile`, `collisions` (Task 12).
- Produces:
  - `pub struct KerningParams { pub strength: f64, pub max_ratio: f64, pub min_value: f64, pub min_distance_ratio: f64, pub group_threshold_ratio: f64, pub exception_threshold: f64, pub grouping: bool, pub lines: usize, pub depth_ratio: f64 }`. `Default` = `{ 0.8, 0.25, 10.0, 0.02, 0.03, 10.0, true, 64, 0.15 }`.
  - `pub enum PlannedRef { Glyph(GlyphId), Group(String) }`.
  - `pub struct PlannedGroup { pub name: String, pub side: KernSide, pub members: Vec<GlyphId> }`.
  - `pub struct PlannedPair { pub first: PlannedRef, pub second: PlannedRef, pub value: f64 }`.
  - `pub struct KerningPlan { pub groups: Vec<PlannedGroup>, pub pairs: Vec<PlannedPair> }`.
  - `pub fn auto_kerning(font: &Font, master: MasterId, glyphs: &[GlyphId], params: &KerningParams) -> Result<KerningPlan, SpacingError>`, con el algoritmo de §6.2:
    - hueco óptico y referencia `HH`;
    - `kern = clamp((ref − hueco)·strength, −max, +max)`;
    - se descarta si `|kern| < min`;
    - guardia de colisión hasta `min_distance`;
    - redondeo a entero.
    - **Agrupación:**
      - lados derechos (lado `First`) y lados izquierdos (`Second`) por separado;
      - cluster greedy en orden de nombre: un glifo entra en el primer grupo cuyo representante (el primer miembro) difiere ≤ `group_threshold_ratio·UPM` en L1 medio por línea;
      - el nombre del grupo es el nombre del representante;
      - los grupos de 1 miembro no se crean.
    - **Valores de clase:**
      - mediana de los pares de los miembros;
      - excepción por glifo cuando difiere > `exception_threshold`;
      - se omiten los pares de clase con |v| < `min`.
    - Determinista.

- [ ] **Step 1: Tests que fallan:**
  - `av_pair_gets_negative_kern`: con glifos `A` y `V` sintéticos (triángulos) y `H` de referencia → `A/V < -10`.
  - `hh_gets_no_kern`.
  - `kerning_never_creates_collisions` (proptest, 200 casos de pares de polígonos aleatorios): `collisions(...)` tras aplicar el plan da 0.
  - `kerning_is_deterministic`.
  - `similar_glyphs_are_grouped`: `O` y `C` (lado izquierdo redondo) → mismo grupo `Second`.
  - `exceptions_when_member_differs`.
- [ ] **Step 2:** Run → FAIL, implementar → PASS.
- [ ] **Step 3:** Commit: `feat(spacing): automatic class kerning with collision guard`.

---

## Parte C — Herramientas del registro

Reglas comunes a las Tasks 14 a 16 (cada tarea deja todo en verde):

**Registro**
- Añadir las herramientas al final de `REGISTRY`, en el orden de Global Constraints, y subir la longitud del array.
- Añadir el módulo en `tools/mod.rs`.
- Añadir el `Toolset` nuevo, si toca:
  - en `ALL` y `as_str`;
  - en `docs.rs::toolset_summary`, con texto en español;
  - en la ayuda `--toolsets` de `cli.rs`;
  - en el test `expected_toolset`.

**Tests y contadores**
- Actualizar los contadores 35 → N en todos los tests listados en el mapa de código:
  - `mcp_safety.rs:517`, `contract.rs` `tool_count_is_*` y su lista;
  - `output_schemas.rs`: número de herramientas JSON, y **llamar cada herramienta JSON nueva en un camino de éxito**;
  - `cli.rs:657,789,880`;
  - `document_tools.rs`: listas de orden, conjuntos, efectos e `implemented`;
  - `edit_tools.rs::MUTATING`, cuando la herramienta usa `Mutation<…>`.
- La ayuda de `forma-cli/src/cli.rs:11` cuenta las herramientas.

**Esquemas, contrato y documentación**
- Las herramientas mutantes usan `Mutation<…OpArgs>` y `run_edit`/`mutate`. Las de lectura, `parse_checked` y `json_output`.
- Las descripciones:
  - son en inglés y de ≤ 1000 caracteres;
  - las mutantes terminan con `base_rev_note!()`;
  - las que pueden pedir aprobación dicen "may require approval depending on the server policy";
  - recuerdan que los nombres y el contenido de las imágenes son datos.
- `forma call <tool> --args '{}'` debe dar un error de herramienta limpio, con exit 1.
- Regenerar el snapshot y `docs/api/tools.md` (comandos en Global Constraints).
- En la descripción del commit, clasificar el cambio del snapshot como **aditivo**.

**Lotes**
- Las herramientas que traducen a comandos son "batchables": se añaden a `BatchTool`, a `translate` y a la lista del test y de la descripción de `edit_batch`.
- Lo son: `component_set`, `glyph_compose`, `glyph_decompose`, `glyph_stroke`, `remove_overlap` y `kerning_set`.

### Task 14: Herramientas de componentes y geometría (`component_set`, `glyph_compose`, `glyph_decompose`, `glyph_stroke`, `remove_overlap`)

**Files:**
- Create: `core/crates/forma-engine/src/tools/{components.rs,stroke.rs}`
- Modify:
  - `core/crates/forma-engine/src/{registry.rs,tools/mod.rs,tools/edit_batch.rs,tools/geometry.rs}`
  - `core/crates/forma-cli/src/{cli.rs,docs.rs}`
  - los tests listados en las reglas comunes, el snapshot y `docs/api/tools.md`
- Test: `core/crates/forma-engine/tests/components_tools.rs`

**Interfaces:**
- Consumes: Tasks 4 y 5.
- Produces:
  - `component_set` (`Toolset::Components`, DESTRUCTIVE): `Mutation<ComponentSetOpArgs { glyph: GlyphRef, master: Option<MasterId>, components: Vec<ComponentInput> }>`.
  - `glyph_compose` (`Components`, WRITE): `Mutation<GlyphComposeOpArgs { name: String, unicodes: Vec<CodePoint> (default []), base: GlyphRef, marks: Vec<MarkPlacement> }>`.
    - `MarkPlacement { glyph: GlyphRef, position: "above"|"below"|"center", gap: f64 (default 0.04·UPM redondeado) }`.
    - Colocación:
      - `above`: centro x del bbox de la marca = centro x del bbox de la base; y-min de la marca = y-max de la base + `gap`.
      - `below`: simétrico.
      - `center`: centrado en x e y.
      - Todo con `layer_bbox` (componentes incluidos).
    - Se traduce a `CreateGlyph { advance_width = avance de la base }` + `SetComponents` (la base en dx = 0 y las marcas con su desplazamiento) en una transacción.
    - Resultado: `MutationResult` + `id` del glifo nuevo, como `glyph_create`.
  - `glyph_decompose` (`Components`, WRITE): `{ glyphs: Vec<GlyphRef>, master }` → `Decompose`.
  - `glyph_stroke` (`Glyphs`, DESTRUCTIVE): `Mutation<GlyphStrokeOpArgs { glyph: GlyphRef, master: Option<MasterId>, skeletons: Vec<Skeleton>, width: f64, cap: Cap, mode: "replace"|"append" (default "replace") }>`.
    - Calcula `stroke` en el motor.
    - `replace` → `SetOutline` (destructivo si la capa tiene dibujo); `append` → `ContourEdit::Add` por contorno, seguido de `RemoveOverlap`.
    - Los nodos del esqueleto cuentan contra `MAX_NODES` y, además, ≤ 10 000 → `LIMIT_EXCEEDED`.
    - Si el glifo no existe → `NOT_FOUND`: se crea antes con `glyph_create` (sin contornos).
  - `remove_overlap` (`Geometry`, WRITE): `{ glyphs: Vec<GlyphRef>, master }` → `RemoveOverlap`.

- [ ] **Step 1: Tests que fallan** (`components_tools.rs`):
  - `compose_aacute_places_mark_above`: el y-min de la marca = cap + gap ± 1, y está centrada ± 1.
  - `compose_result_has_id_and_components`.
  - `component_set_replacing_requires_plan`: con la política `prompted`, la segunda llamada sobre componentes existentes → `APPROVAL_REQUIRED`.
  - `decompose_tool_matches_render`.
  - `stroke_tool_creates_clean_outline`: `geometry_check` vacío.
  - `stroke_append_unions_with_existing`.
  - `stroke_degenerate_is_invalid_argument` (Review Focus 5).
  - `stroke_node_limit`.
  - `remove_overlap_noop_returns_null_tx` (Review Focus 5).
  - `batch_can_compose_and_stroke`: un `edit_batch` con `glyph_stroke` + `glyph_compose`.
- [ ] **Step 2:** Run `cargo test -p forma-engine --locked` → FAIL. Implementar y actualizar los contadores, el snapshot y la documentación según las reglas comunes.
- [ ] **Step 3:** Run `cargo test --workspace --locked` → PASS (40 herramientas). Clippy limpio.
- [ ] **Step 4:** Commit: `feat(engine): component, compose, decompose, stroke and overlap tools`. En el cuerpo: "tools/list snapshot: additive (5 tools, components toolset)".

### Task 15: Herramientas de espaciado, kerning y juego de caracteres

**Files:**
- Create: `core/crates/forma-engine/src/tools/{kerning.rs,charset.rs}`
- Modify:
  - `core/crates/forma-engine/src/{registry.rs,tools/mod.rs,tools/metrics.rs,tools/edit_batch.rs}`
  - `core/crates/forma-engine/Cargo.toml` (`forma-spacing`)
  - `forma-cli` (docs y ayuda), los tests de las reglas comunes, el snapshot y la documentación
- Test: `core/crates/forma-engine/tests/kerning_tools.rs`

**Interfaces:**
- Consumes: Tasks 8, 12 y 13.
- Produces:
  - `spacing_auto` (`Spacing`, WRITE): `Mutation<SpacingAutoOpArgs { master: Option<MasterId>, glyphs: Option<Vec<GlyphRef>>, tightness: Option<f64> (0.05..=3.0), references: Option<[GlyphRef; 2]> }>`.
    - Calcula `auto_spacing` y emite un `SetMetrics { lsb, rsb }` por glifo.
    - Es masiva si toca más de 20 glifos.
  - `kerning_get` (`Kerning`, READ): `{ doc, master: Option<MasterId>, glyphs: Option<Vec<GlyphRef>> }`.
    - Devuelve `{ rev, groups: Bounded<{name, side, members: [names]}>, pairs: Bounded<{first, second, value}> }`, con las referencias como nombres de glifo o de grupo (`@group`).
  - `kerning_set` (`Kerning`, DESTRUCTIVE): `Mutation<KerningSetOpArgs { master: Option<MasterId>, groups: Vec<{name, side, members}> (default []), delete_groups: Vec<{name, side}> (default []), pairs: Vec<{first: KernRefInput, second: KernRefInput, value: f64}> (default []) }>`.
    - Los comandos van en ese orden: grupos, borrados y pares.
    - Al menos uno no vacío; si no, `INVALID_ARGUMENT`.
  - `kerning_auto` (`Kerning`, DESTRUCTIVE): `Mutation<KerningAutoOpArgs { master, glyphs: Option<Vec<GlyphRef>>, strength, min_value, max_ratio, grouping: Option<bool>, replace: bool (default true) }>`.
    - Con `replace`, primero quita los pares y grupos existentes de ese master (destructivo si existían) y después aplica el plan.
    - Es masiva si hay más de 20 entidades.
  - `kerning_check` (`Kerning`, READ): `{ doc, master, glyphs: Option<Vec<GlyphRef>>, min_distance: Option<f64> }`.
    - Por defecto, todos los pares de los glifos exportados con contorno o componentes.
    - Más de `MAX_CHECK_PAIRS` → `LIMIT_EXCEEDED`.
    - Devuelve `{ rev, checked_pairs, collisions: Bounded<{left, right, distance}> }`.
  - `charset_check` (`Quality`, READ): `{ doc, profile: Option<String>, codepoints: Option<Vec<CodePoint>> }`.
    - Exactamente uno de los dos; un perfil desconocido → `INVALID_ARGUMENT`.
    - Devuelve `{ rev, profile, required, covered, coverage: f64 (0..=1), missing: Bounded<String "U+XXXX c">, composite: Bounded<String>, outlined: Bounded<String> }`.
  - Toolset nuevo `kerning`.

- [ ] **Step 1: Tests que fallan:**
  - `spacing_auto_sets_symmetric_h`.
  - `spacing_auto_mass_needs_plan`: 25 glifos con `prompted` → `APPROVAL_REQUIRED`; con `dry_run`, devuelve el plan.
  - `kerning_set_and_get_roundtrip`.
  - `kerning_set_group_conflict_is_invalid_argument`.
  - `kerning_auto_then_check_has_no_collisions` (criterio 5, motor).
  - `kerning_auto_replace_existing_is_destructive`.
  - `charset_check_unicase_es_reports_missing`.
  - `charset_requires_exactly_one_selector`.
- [ ] **Step 2:** Run → FAIL. Implementar y actualizar los contadores, el snapshot y la documentación.
- [ ] **Step 3:** Run `cargo test --workspace --locked` → PASS (46 herramientas). Clippy limpio.
- [ ] **Step 4:** Commit: `feat(engine): spacing, kerning and charset tools`, con "tools/list snapshot: additive (6 tools, kerning toolset)".

### Task 16: Herramientas de trazado (`image_inspect`, `image_trace`, `trace_compare`, `glyph_measure`)

**Files:**
- Create: `core/crates/forma-engine/src/tools/trace.rs`
- Modify:
  - `core/crates/forma-engine/src/{registry.rs,tools/mod.rs,errors.rs,tools/capabilities.rs}`
  - `core/crates/forma-engine/Cargo.toml` (`forma-trace`)
  - `core/crates/forma-mcp/src/*` (instrucciones del servidor)
  - `forma-cli` (docs y ayuda), los tests de las reglas comunes, el snapshot y la documentación
- Test: `core/crates/forma-engine/tests/trace_tools.rs`

**Interfaces:**
- Consumes: Tasks 9 a 11 (la imagen de prueba propia como fixture).
- Produces:
  - `errors.rs::from_trace_error`:
    - `TooLarge` y `TooManyCandidates` → `LIMIT_EXCEEDED`;
    - `Decode`, `CandidateOutOfRange` y `Vectorize` → `INVALID_ARGUMENT`;
    - `Render` y `Geometry` → `INVALID_GEOMETRY`.
  - Lectura de imagen común: `engine.check_allowed(path, true)` → `fs::metadata` (si el tamaño supera `MAX_IMAGE_BYTES`, `LIMIT_EXCEEDED` sin leer) → `fs::read` → `forma_trace::decode`. El argumento se llama `path`, para que la CLI lo añada como raíz.
  - `image_inspect` (`Trace`, READ, salida Image): `{ path, color: Option<String "#RRGGBB">, tolerance: Option<f64> (1..=100) }` → PNG anotado + resumen `Analysis` (sin `doc`).
  - `image_trace` (`Trace`, DESTRUCTIVE): `Mutation<ImageTraceOpArgs { path, assignments: Vec<{candidate: usize, name: String, unicodes: Vec<CodePoint> (default [])}> (1..=200), color, tolerance, master: Option<MasterId> }>`.
    - `cap_height` = el del master.
    - Por cada asignación: si el glifo no existe, `CreateGlyph` con los contornos y el avance; si existe, `SetOutline` + `SetMetrics { advance_width }`, que es destructivo si tenía dibujo.
    - Resultado: `MutationResult` + `traced: [{name, candidate, iou, nodes}]`.
  - `trace_compare` (`Trace`, READ, Image): `{ doc, path, glyph: GlyphRef, candidate: usize, color, tolerance, master }` → PNG de superposición + `{ rev, glyph, candidate, iou }`.
  - `glyph_measure` (`Geometry`, READ): `{ doc, glyph: GlyphRef, master }` → `{ rev, glyph, stroke_median, stroke_p10, stroke_p90, terminal_radius, join_min }`.
  - `forma_capabilities` informa de los límites nuevos (imagen, candidatos, asignaciones y pares de `kerning_check`).
  - Las instrucciones del servidor añaden el flujo imagen → fuente de §8 de la especificación, en inglés, en ≤ 12 líneas.
  - Toolset nuevo `trace`.

- [ ] **Step 1: Tests que fallan** (con el `specimen.png` de la Task 11, copiado a la raíz permitida del test):
  - `inspect_returns_annotated_png_and_eight_candidates`.
  - `trace_creates_glyphs_with_iou`.
  - `trace_existing_glyph_needs_plan`.
  - `trace_candidate_out_of_range_is_invalid_argument` (Review Focus 1).
  - `inspect_outside_allowed_dir_is_path_not_allowed`.
  - `oversized_file_is_limit_exceeded_without_reading`: un archivo disperso de 65 MiB.
  - `trace_compare_reports_iou`.
  - `glyph_measure_reports_stroke`.
  - `capabilities_report_image_limits`.
- [ ] **Step 2:** Run → FAIL. Implementar y actualizar los contadores, el snapshot y la documentación.
- [ ] **Step 3:** Run `cargo test --workspace --locked` → PASS (50 herramientas). Clippy limpio.
- [ ] **Step 4:** Commit: `feat(engine): image inspection, tracing, comparison and style measurement tools`, con "tools/list snapshot: additive (4 tools, trace toolset)".

### Task 17: Escenario MCP de punta a punta del Hito 2

**Files:**
- Create: `core/crates/forma-cli/tests/mcp_scenario_hito2.rs`
- Modify: `core/crates/forma-cli/tests/mcp_safety.rs` (casos de imagen y sandbox), `core/crates/forma-cli/tests/mcp_common/mod.rs` (helpers que se reutilicen)

**Interfaces:**
- Consumes: Tasks 14 a 16 y el fixture de la Task 11.
- Produces: el test `image_to_font_scenario(#[case] mode)` en `legacy()` y `modern()`, con un timeout de 120 s. Pasos:
  1. `document_create` "Specimen Demo" (UPM 1000, cap 700).
  2. `image_inspect` → 8 candidatos.
  3. `image_trace` con las asignaciones `0..8 → A B C H O F M R`, los unicodes `[U+0041, U+0061]`, etc. Cada `iou ≥ 0.97`.
  4. `geometry_check` → 0 problemas.
  5. `glyph_measure` sobre `H` → `stroke_median > 0`.
  6. `glyph_stroke`: `acutecomb`, creado antes con `glyph_create`, con un esqueleto diagonal y `Bulb{ratio:1.4}` del ancho medido × 0,8; y `zero`, con un esqueleto ovalado cerrado de ancho igual al medido.
  7. `glyph_compose` de `Aacute` (U+00C1, U+00E1) y `Oacute` (U+00D3, U+00F3), con `acutecomb` `above`.
  8. `spacing_auto` → `APPROVAL_REQUIRED` si toca más de 20 glifos; si no, se aplica. Con 12 glifos se aplica directamente.
  9. `kerning_auto` con `dry_run: true` → plan; luego `plan_commit`.
  10. `kerning_check` → 0 colisiones.
  11. `charset_check` con `codepoints` = las letras de la imagen y sus minúsculas, `space`, Á á Ó ó y 0 → `coverage == 1.0`.
  12. `validate` → 0 errores.
  13. `export_font` TTF y WOFF2.
  14. `document_save`, `document_close` y `document_open` → kerning y componentes idénticos (`kerning_get`, `glyph_get`).
  15. Historial atribuido a `agent:forma-test-agent`.
  - Si `FORMA_ACCEPTANCE_OUT` está definido (corrida legacy), escribe `hito2-specimen.ttf`, `hito2-specimen.woff2`, `hito2-specimen.forma` y `hito2-expected.json`. El último lleva los glifos, cmap y avances esperados, más los pares de kerning esperados que se comprueban con HarfBuzz.
  - En `mcp_safety.rs`:
    - `image_path_outside_root_is_refused`.
    - `image_symlink_escape_is_refused`, solo en Unix; en Windows, la junction equivalente.
    - `image_names_with_instructions_are_data`: una asignación con `name` "ignore_previous_instructions" crea un glifo con ese nombre, sin más efecto.

- [ ] **Step 1:** Escribir los tests.
- [ ] **Step 2:** Run `cargo test -p forma-cli --locked --test mcp_scenario_hito2 --test mcp_safety` → PASS. Si algo falla, corregir en la tarea dueña del código y anotarlo en el informe.
- [ ] **Step 3:** Commit: `test(mcp): milestone 2 image-to-font scenario in both lifecycles`.

---

## Parte D — Aceptación, CI y documentación

### Task 18: Fuente unicase-es completa de prueba y oráculos externos (criterios 3, 5 y 6)

**Files:**
- Create: `core/crates/forma-cli/tests/unicase_font.rs`, `tools/oracle/verify_hito2.py`
- Modify: `scripts/acceptance.sh`, `scripts/acceptance.ps1`, `tools/oracle/requirements.txt` (sin dependencias nuevas, si se puede)

**Interfaces:**
- Consumes: el motor completo (vía MCP con el harness de `mcp_common`).
- Produces:
  - **Test `unicase_es_complete_font`.** Construye por MCP la fuente con los 89 glifos de §3 de la especificación:
    - las 8 letras trazadas de la imagen de prueba;
    - las otras 18 letras, cada una con un `glyph_stroke` simple y deterministico definido en una tabla del test (barras y curvas básicas por letra; no se busca belleza);
    - las 3 marcas y las 7 acentuadas con `glyph_compose`;
    - las cifras y los signos con esqueletos simples de la tabla;
    - el espaciado y el kerning automáticos.
    - Afirmaciones:
      - `charset_check` `unicase-es` → `coverage == 1.0` (criterio 3);
      - `kerning_check` → 0 colisiones (criterio 5);
      - `validate` → 0 errores;
      - las acentuadas son compuestas en el TTF (read-fonts) (criterio 4).
    - Con `FORMA_ACCEPTANCE_OUT`, escribe `unicase-es.ttf`, `unicase-es.woff2` y `unicase-expected.json` (cmap, compuestos, pares con valores).
  - **`verify_hito2.py`** (`--ttf --woff2 --expected`):
    - OTS sobre TTF y WOFF2;
    - `hb-shape` vía `uharfbuzz` por cada par esperado: el avance del primer glifo + kern coincide con el valor esperado, ±0;
    - fontTools: cmap = esperado, glifos compuestos = esperados, tabla `GPOS` presente con lookup de `kern`;
    - en Linux, si existe `google-chrome` o `chromium`, carga el WOFF2 en una página con `@font-face` y `document.fonts.check` mediante `--headless=new --dump-dom`, y espera `true`. En otro SO imprime `SKIPPED`.
    - Termina con exit ≠ 0 si algo falla.
  - **`acceptance.sh` y `acceptance.ps1`:** pasos nuevos tras los del Hito 1:
    1. `FORMA_ACCEPTANCE_OUT=$out cargo test -p forma-cli --locked --test mcp_scenario_hito2 --test unicase_font`;
    2. `verify_hito2.py` sobre `hito2-specimen.*` y `unicase-es.*`;
    3. fontspector (perfil `opentype`, `--error-code-on fail`) sobre los dos TTF.
    - El mensaje final sigue siendo `ACCEPTANCE OK`.

- [ ] **Step 1:** Escribir el test y el script.
- [ ] **Step 2:** Run `scripts/acceptance.sh` (macOS local, con `tools/oracle/.venv`) → `ACCEPTANCE OK`, Chrome incluido si está instalado. Si fontspector reporta un FAIL, corregir la causa en el motor (en la tarea correspondiente) y no relajar el criterio. Los WARN se anotan en el informe.
- [ ] **Step 3:** Commit: `test(acceptance): complete unicase-es font and milestone 2 oracles`.

### Task 19: CI, cobertura y fuzzing

**Files:**
- Modify: `.github/workflows/ci.yml`, `.github/workflows/nightly.yml`, `core/fuzz/{Cargo.toml,fuzz_targets/image_decode.rs}`
- Create: `core/fuzz/seeds/image_decode/{tiny.png,tiny.jpg}`

**Interfaces:**
- Produces:
  - **Coverage gate:** `--min 80 forma-model forma-edit forma-engine forma-store forma-trace forma-spacing`.
  - **Fuzz:** objetivo `image_decode`, que hace `forma_trace::decode(data)` y, si decodifica, `analyze` con las opciones por defecto. Nunca debe dar pánico ni agotar memoria. `forma-fuzz` depende de `forma-trace`.
  - **`nightly.yml`:** matriz `[load_package, load_forma, image_decode]`, 900 s cada uno.
  - **`ci.yml`:** en Ubuntu, el paso de aceptación tiene Chrome disponible (`google-chrome` viene en la imagen `ubuntu-24.04`; verificarlo con `google-chrome --version` en el paso). Si faltara, instalarlo con `apt`, de forma justificada.
- [ ] **Step 1:** Medir la cobertura en local: `cargo llvm-cov --workspace --locked --json --summary-only --output-path target/llvm-cov-summary.json && python3 -I ../scripts/coverage_gate.py target/llvm-cov-summary.json --min 80 forma-model forma-edit forma-engine forma-store forma-trace forma-spacing` → todas ≥ 80 %. Si alguna crate queda por debajo, añadir tests en esa crate (no excluir código).
- [ ] **Step 2:** Compilar el objetivo de fuzz: `cargo +nightly fuzz build image_decode` (desde `core/fuzz`) y correrlo 60 s en local: `cargo +nightly fuzz run image_decode -- -max_total_time=60` → sin fallos.
- [ ] **Step 3:** Commit: `ci: gate coverage of trace and spacing, fuzz the image decoder nightly`.

### Task 20: Documentación del hito

**Files:**
- Modify:
  - `docs/07-mcp-ai-architecture.md`: catálogo con las 50 herramientas, conjuntos y flujo imagen → fuente.
  - `docs/guides/connect-agents.md`: sección "De una imagen a una fuente", con un prompt de ejemplo y la advertencia sobre los derechos de las imágenes.
  - `README.md` y `README.es.md`: 35 → 50 herramientas, tabla del roadmap nueva (Hito 2 en curso, UI al final), y fila "Imagen → fuente" en "What works today".
  - `docs/03-architecture.md`: crates nuevas, componentes y kerning en el modelo, formato v2.
- Create: `docs/qa/hito2/README.md` (esqueleto que completa el controlador en la Task 21), `docs/qa/hito2/decisiones.md` (vacío, con encabezado).

- [ ] **Step 1:** Escribir los cambios. Cada número y cada nombre se verifica contra el código, al estilo groundtruth: contar herramientas con `forma call forma_capabilities` y listar conjuntos con `forma mcp --help`.
- [ ] **Step 2:** `cargo test -p forma-cli --locked api_docs_up_to_date` → PASS.
- [ ] **Step 3:** Commit: `docs: milestone 2 tools, architecture, guides and README`.

### Task 21: Demo con agente real sobre la imagen propia (controlador; criterio 8, parte publicable)

Ejecuta el **controlador**, no un subagente.

- [ ] **Step 1:** `cargo install --path core/crates/forma-cli --locked`. Copiar `tests/fixtures/hito2/specimen.png` a una carpeta temporal de demo.
- [ ] **Step 2:** `claude -p` con `forma mcp --allow-dir <demo>`, sin otras herramientas, con la instrucción: "Using only the forma MCP tools, turn the alphabet in specimen.png into a complete font called 'Specimen Demo': trace every letter, design the digit 0 and the marks you need in the same style, compose Á and Ó, space and kern it, check it has no collisions, validate it with zero errors, export TTF and WOFF2, save it, and show me a preview of 'HOLA ÁRBOL 0'."
- [ ] **Step 3:** Verificar el TTF con `verify_hito2.py` y fontspector. Guardar la transcripción condensada, el preview y el informe en `docs/qa/hito2/`, sin metadatos locales.
- [ ] **Step 4:** Commit: `docs(qa): milestone 2 demo with a real agent on the own specimen`.

### Task 22: Demo con el alfabeto de la hermana (controlador; criterio 8, local)

Ejecuta el **controlador**. **Nada de esta tarea entra en el repositorio.**

- [ ] **Step 1:** Pedir al responsable la ruta de la imagen (por ejemplo `~/fonts/hermana/alfabeto.png`). Si no está disponible, la tarea queda pendiente y se anota en el informe final: no bloquea el resto.
- [ ] **Step 2:** `claude -p` con `forma mcp --allow-dir ~/fonts/hermana`, con la instrucción "convierte este alfabeto en una fuente completa unicase-es" y el detalle del perfil.
- [ ] **Step 3:** Hoja de prueba: `preview_text` de todas las líneas del perfil. Entregar al responsable `~/fonts/hermana/{fuente.forma,fuente.ttf,fuente.woff2,hoja-de-prueba.png}` para su aprobación (y la de la autora).
