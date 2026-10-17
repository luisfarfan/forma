# Hito 2 — De una imagen a una fuente completa — Diseño

> Estado: aprobado por el responsable en tres partes durante la conversación del 2026-10-17. Pidió además ejecutarlo completo sin más pausas ("hazlo de una vez, todo completo").
> Rama: `hito2`. Sustituye al antiguo Hito 2 (spike de UI), que pasa al final del roadmap (ADR 0008).

## 1. Contexto y decisión de roadmap

El Hito 1 dejó un núcleo sin interfaz, con CLI y servidor MCP, verificado en macOS, Windows y Ubuntu. El responsable decidió:

1. **Motor primero, editor gráfico al final.** Todo lo que hace falta para crear una fuente perfecta (kerning, componentes, variables, features OpenType, CFF, hinting, importación) va antes que la interfaz.
2. **Hitos guiados por fuentes demo.** Cada hito se define por una fuente concreta que FORMA debe poder crear, y se construye lo que esa fuente exige.
3. **Nada de fuentes comerciales.** Las demostraciones usan dibujos con derechos claros, muestras de dominio público y fuentes OFL. Las comerciales solo sirven como referencia visual.

**Fuente demo del Hito 2:** el alfabeto que dibujó la hermana del responsable. Son las mayúsculas A–Z en un estilo display "líquido": un trazo de grosor casi constante, terminales hinchados en forma de bola y uniones estrechadas, en naranja sobre negro. La imagen incluye además otros elementos: una línea "{ALPHABETICAL}" y figuras azules.

- **Solo existe esta imagen**, de unos 1370 × 911 px, con letras de unos 110 px de alto.
- La imagen y la fuente resultante se quedan **en local, fuera del repositorio**. Solo se publican si la autora lo autoriza, con su nombre y la licencia que elija.

## 2. Objetivo y entregas

**Objetivo:** a partir de la imagen de un alfabeto, FORMA (manejado por un agente vía MCP, o por la CLI) produce una fuente unicase completa, espaciada, con kerning, validada y lista para la web.

| Entrega | Resultado usable |
|---|---|
| **2a. Del dibujo a la fuente** | Imagen → detectar y asignar letras → vectorizar → métricas → espaciado automático → kerning automático. Una fuente que ya escribe cualquier texto, porque cada minúscula apunta a su mayúscula. |
| **2b. Completar el alfabeto** | Números, puntuación y símbolos dibujados en el mismo estilo con el trazo de esqueleto, la unión de contornos y la medición del estilo. Acentos españoles como componentes. Cobertura completa del perfil `unicase-es`. |

## 3. Juego de caracteres `unicase-es` (exacto)

| Grupo | Glifos (nombre → códigos) | Cantidad |
|---|---|---|
| Básicos | `.notdef` (sin código); `space` → U+0020, U+00A0 | 2 |
| Letras | `A` → U+0041, U+0061 … `Z` → U+005A, U+007A (cada mayúscula también cubre su minúscula) | 26 |
| Acentuadas (componentes) | `Aacute` → U+00C1, U+00E1; `Eacute` → U+00C9, U+00E9; `Iacute` → U+00CD, U+00ED; `Oacute` → U+00D3, U+00F3; `Uacute` → U+00DA, U+00FA; `Udieresis` → U+00DC, U+00FC; `Ntilde` → U+00D1, U+00F1 | 7 |
| Marcas (solo componentes, sin código) | `acutecomb`, `dieresiscomb`, `tildecomb` | 3 |
| Cifras | `zero` … `nine` → U+0030 … U+0039 | 10 |
| Puntuación y símbolos | `period` . · `comma` , · `colon` : · `semicolon` ; · `exclam` ! · `exclamdown` ¡ · `question` ? · `questiondown` ¿ · `quotesingle` ' · `quotedbl` " · `quoteleft` ‘ · `quoteright` ’ · `quotedblleft` “ · `quotedblright` ” · `hyphen` - · `endash` – · `emdash` — · `parenleft` ( · `parenright` ) · `bracketleft` [ · `bracketright` ] · `braceleft` { · `braceright` } · `slash` / · `backslash` \ · `ampersand` & · `at` @ · `numbersign` # · `dollar` $ · `percent` % · `asterisk` * · `plus` + · `equal` = · `underscore` _ · `less` < · `greater` > · `ellipsis` … · `periodcentered` · · `guillemotleft` « · `guillemotright` » · `Euro` € | 41 |
| **Total** | | **89 glifos, 119 códigos** |

`charset_check` con el perfil `unicase-es` comprueba los 119 códigos. Los nombres de glifo son una recomendación (AGLFN), no un requisito del chequeo.

## 4. Criterios de aceptación

Todos están automatizados en la CI de los 3 SO, salvo el 8, que es la demo manual.

1. **Fidelidad del trazado.** Sobre la imagen de prueba propia (§10.2), cada letra vectorizada, rasterizada a la escala de la imagen, coincide con la máscara de origen con **IoU ≥ 0,97**. Contra el contorno real conocido, el IoU por área en unidades de fuente también es **≥ 0,97**.
2. **Contornos limpios.** Tras `image_trace`, el glifo pasa `geometry_check` sin problemas (dirección, extremos, duplicados, segmentos diminutos, grilla), no tiene contornos solapados y no tiene autointersecciones.
3. **Cobertura.** Un test construye una **fuente unicase-es completa** de prueba. Tiene los 89 glifos: letras de la imagen de prueba más letras y signos simples dibujados con `glyph_stroke`, y las acentuadas con `glyph_compose`. En ella `charset_check` (`unicase-es`) da el 100 % de los códigos. Esa misma fuente se usa en los criterios 5 y 6.
4. **Acentos por componentes.** `Aacute` … `Ntilde` son componentes (base + marca), no contornos copiados. Cambiar la base cambia la acentuada (test del motor), y el TTF los contiene como glifos compuestos (fontTools).
5. **Espaciado y kerning.**
   - `spacing_auto` da márgenes simétricos (±1 unidad) a un glifo simétrico.
   - `kerning_auto` produce pares que HarfBuzz aplica con los valores del modelo (`hb-shape`, test numérico).
   - `kerning_check` sobre todos los pares del juego de caracteres reporta **0 colisiones** (ningún par se toca ni se solapa con el kerning aplicado).
6. **Salida perfecta.** `validate` da 0 errores, OTS 0 errores y fontspector (perfil `opentype`) 0 FAIL. El WOFF2 se carga en Chrome (test con navegador sin interfaz en CI Linux).
7. **Disciplina del motor.**
   - Toda funcionalidad es una herramienta del registro, expuesta por MCP y CLI, con test de paridad.
   - Los schemas están en el snapshot de contrato.
   - Hay un escenario MCP de punta a punta (§10.4) en los dos ciclos de vida del protocolo.
   - Cobertura ≥ 80 % en `forma-trace` y `forma-spacing` (y se mantiene en las crates del Hito 1).
   - El lector de imágenes entra en el fuzzing nocturno.
   - CI en verde en los 3 SO.
8. **Demo real (manual, local).** Claude Code, conectado a `forma mcp`, recibe la imagen de la hermana y la instrucción "convierte este alfabeto en una fuente completa", y produce la fuente.
   - La fuente cumple los criterios 2–6.
   - El responsable aprueba la hoja de prueba de los glifos diseñados en la 2b.
   - La evidencia queda en local (`~/fonts/hermana/`). En `docs/qa/hito2/` solo se publica la demo con la imagen de prueba propia.

## 5. Modelo de datos (formato v2)

### 5.1 Componentes
- `Component { base: GlyphId, transform: Transform }`, con `Transform { xx, xy, yx, yy, dx, dy: f64 }` (afín, identidad por defecto).
- `Layer` gana `components: Vec<Component>`, que se serializa vacío por omisión, para compatibilidad.
- **Invariantes** (en `Font::validate_invariants`):
  - la base existe y tiene capa en ese master;
  - no hay ciclos;
  - la transformación es finita y no singular (|det| ≥ 1e-6).
- Un componente forma parte de la entidad `Glyph` de su glifo, así que **no hay `EntityKey` nueva**.
- Borrar un glifo que otros usan como base da `INVALID_ARGUMENT` con la lista de glifos que dependen de él. El agente debe descomponerlos primero.

### 5.2 Kerning
- **Grupos:** `KerningGroup { id: KerningGroupId, name: String, side: KernSide (First|Second), members: Vec<GlyphId> }`.
  - Un glifo pertenece como mucho a un grupo por lado.
  - Los nombres son únicos por lado y siguen las mismas reglas que los nombres de glifo.
  - Al exportar se convierten en `public.kern1.<name>` / `public.kern2.<name>`.
- **Pares:** `KerningPair { master: MasterId, first: KernRef, second: KernRef, value: f64 }`, con `KernRef = Glyph(GlyphId) | Group(KerningGroupId)`.
  - El valor es finito, |v| ≤ 2 × UPM, y un valor 0 equivale a no tener el par.
- **Entidades (concurrencia por entidad):**
  - `EntityKey::KerningGroup(KerningGroupId)`, en texto `kerning_group:<uuid>`;
  - `EntityKey::KerningPair(MasterId, KernRef, KernRef)`, en texto `kerning_pair:<master>:<ref>:<ref>`, con `ref` = `g.<uuid>` o `k.<uuid>`.
  - Dos actores que ajustan pares distintos no entran en conflicto.
- **Invariantes:** las referencias existen, los grupos están en el lado correcto del par y los miembros existen.
- **Borrado en cascada:** borrar un glifo lo quita de sus grupos y elimina sus pares directos dentro de la misma transacción. Las entidades afectadas cuentan como tocadas.

### 5.3 Persistencia
- `.forma`: `user_version = 2`.
  - Las filas nuevas siguen el mismo esquema de entidades.
  - La **migración v1 → v2** es pura y testeada (agrega componentes vacíos, sin kerning).
  - Un documento v1 se abre, se migra en memoria y se guarda como v2. Un documento de una versión futura se rechaza con un error claro.
- **El kerning (grupos y pares) es un campo de `Font` (`kerning`).** Viaja dentro del documento de fuente: la fila `font` en `.forma` y `font.json` en `.formapackage`.
  - Se serializa en JSON canónico, con los pares de cada master como una lista ordenada por `(first, second)`.
  - El guardado atómico lo cubre sin archivos nuevos, y git muestra un diff legible por par.
  - La granularidad de persistencia (un documento) no cambia la de concurrencia (una entidad por par).
- Journal: `JournalRecord` gana `kerning`, el kerning completo cuando la transacción toca alguna entidad de kerning.

## 6. Crates

### 6.1 `forma-trace` (nueva): de imagen a contornos
Dependencias: `image` 0.25 (solo PNG y JPEG, sin features por defecto), `visioncortex` 0.9 (vectorización), `forma-model`, `forma-geometry`.

1. **Decodificar** con límites: ≤ 64 MiB de archivo y ≤ 50 megapíxeles. Si se superan, `LIMIT_EXCEEDED`.
2. **Paleta y color de tinta.**
   - El fondo es el color predominante en el borde de la imagen.
   - La paleta se obtiene con k-means (k ≤ 8) sobre una muestra.
   - Por defecto, la tinta es el grupo de colores no-fondo con más píxeles. También se acepta `color` explícito en hex.
3. **Máscara suave.** Cada píxel se valora según su distancia en Lab a la tinta frente al fondo, con tolerancia ΔE configurable (por defecto 25).
   - La máscara se **sobremuestrea ×4** (bilineal) antes de binarizarla al 50 %. Así se recupera precisión subpíxel en letras pequeñas.
   - Los colores que no son ni tinta ni fondo (las figuras azules) quedan fuera.
4. **Candidatos.**
   - Componentes conexos con conectividad 8. Se descarta el ruido: área menor que el 2 % de la mediana.
   - Se agrupan en **filas** por solapamiento vertical y se ordenan de izquierda a derecha.
   - Se **fusionan** las piezas de un mismo glifo, es decir, solapamiento horizontal ≥ 50 % del más estrecho dentro de la fila (puntos, diéresis, ¡).
   - Cada candidato tiene índice, fila, caja en píxeles y área.
5. **Métricas por fila.** La línea base es la mediana de los bordes inferiores y la altura de mayúsculas la mediana de los superiores. Escala = `cap_height` del master en unidades ÷ altura de mayúsculas en píxeles de la fila.
6. **Vectorizar** cada candidato con visioncortex.
   - Parámetros: `PathSimplifyMode::Spline`, umbral de esquina 60°, longitud mínima 4, 10 iteraciones, empalme 45°.
   - Coordenadas de fuente con y hacia arriba: `x = (px − caja.min_x) · escala`, `y = (base_px − py) · escala`.
7. **Limpieza** (forma-geometry):
   - simplificación por reajuste de cúbicas con error ≤ 1,5 unidades (§6.3);
   - dirección (exteriores antihorarios);
   - extremos y marcado `smooth` donde las tangentes difieren ≤ 3°;
   - grilla de 1 unidad y eliminación de duplicados y segmentos diminutos.
8. **Fidelidad.** Se rasteriza el contorno a la escala de origen con `forma-render` y se calcula el IoU contra la máscara binarizada del candidato. También se genera una superposición en PNG: máscara en gris, contorno en naranja y diferencias en rojo.

### 6.2 `forma-spacing` (nueva): espaciado, kerning y colisiones
Usa algoritmos propios a partir de principios publicados. No se copia código de herramientas GPL.

- **Perfiles.**
  - Se toman 64 líneas horizontales en la **zona de medida**: de `−overshoot` a `cap_height + overshoot`, con overshoot = 2 % del UPM.
  - Para cada línea se registra la x mínima y máxima de tinta (relleno no-cero, componentes incluidos).
  - Si un glifo tiene tinta en menos de 8 líneas (punto, comillas), su zona es su propia altura de tinta.
  - Profundidad: el perfil se recorta a `depth` = 15 % del UPM desde el extremo del glifo.
- **Espaciado.**
  - El blanco de un lado es el área entre el perfil recortado y el borde (margen), dividida por la altura de la zona.
  - Referencias: `H` y `O`. El margen de `H` es `tightness × ancho_de_trazo(H)`, con `tightness` 0,6 por defecto, apropiado para display. Ese margen fija el blanco objetivo.
  - Cada lado de cada glifo se resuelve por **bisección** hasta igualar el blanco objetivo, con tolerancia de 0,5 unidades.
  - Propiedad: un glifo simétrico recibe márgenes simétricos.
- **Kerning automático.**
  - Para el par (L, R) el hueco por línea es `g(y) = (avance_L − der_L(y)) + izq_R(y)`, con perfiles recortados, y el **hueco óptico** es la media de `g` en las líneas con tinta en ambos.
  - La referencia es el hueco de `HH`. El kern es `clamp((ref − hueco) × fuerza, −max, +max)`, con `fuerza` 0,8, `max` = 25 % del UPM y `min` = 10 unidades (por debajo, no hay par).
  - **Guardia de colisión:** si `min_y g(y) + kern < distancia_mínima` (2 % del UPM), el kern se sube hasta cumplirla.
  - **Agrupación:** se unen los glifos cuyos perfiles normalizados difieren menos que un umbral (L1 ≤ 3 % del UPM por línea). El kern de clase es la mediana de los miembros, y se crean excepciones donde un miembro difiere más de 10 unidades.
  - El resultado es determinista: orden por nombre.
- **Colisiones.** Para todos los pares (o un subconjunto), se calcula la distancia mínima entre contornos con avance y kerning aplicados. Hay colisión si es menor que 0 (toque o solapamiento).

### 6.3 `forma-geometry` (ampliada)
- **`fit`:** ajuste de cúbicas a puntos (algoritmo de Schneider, implementación propia) con error máximo configurable. Lo usan `forma-trace` y el trazo.
- **`stroke`:**
  - Esqueleto: contornos abiertos o cerrados de nodos línea/curva/offcurve, cada nodo on-curve con su `width` opcional.
  - Estilo: `width` por defecto, `cap = round | bulb { ratio }`, `join = round`.
  - Se muestrea el esqueleto (paso ≤ 2 unidades), se desplaza ±w(t)/2 por la normal con interpolación lineal del ancho entre nodos, se reajusta a cúbicas (error ≤ 0,5) y se agregan las tapas.
  - `bulb` añade un círculo de radio `ratio × w/2` en cada extremo abierto.
  - El resultado se une con `remove_overlap`.
- **`boolean`:**
  - `remove_overlap(paths) -> Vec<Path>`: unión con relleno no-cero vía **linesweeper** 0.5, detrás del trait `PathOps`, para poder cambiar a Skia PathOps si fallara.
  - Después se normaliza la dirección y se agregan extremos.
  - Error `INVALID_GEOMETRY` si linesweeper falla.
- **`decompose`:** convierte los componentes de una capa en contornos con su transformación, de forma recursiva.

### 6.4 Crates existentes
- `forma-edit`: comandos nuevos (§7.2) con efectos y entidades tocadas, y deshacer/rehacer como el resto.
- `forma-interop`: el UFO lleva componentes (`norad::Component`), `kerning.plist` y `groups.plist`.
- `forma-compile`: fontc compila componentes y kerning a `glyf` compuesto y `GPOS kern`. Preflight nuevo: ciclos y bases inexistentes (defensa en profundidad).
- `forma-validate`: chequeos nuevos y perfiles de juego de caracteres (`charset`).
  - Las referencias rotas ya las impiden las invariantes del modelo.
  - `glyph.mixed_contours_components` (aviso: fontc descompone esos glifos).
  - `kerning.unexported_reference` (aviso: el par apunta a un glifo que no se exporta).
  - `binary.gpos` (error: el modelo tiene kerning y el binario no tiene tabla GPOS).
- `forma-render`: renderiza componentes, aplica el kerning del master por defecto en `preview_text` y añade el rasterizado a una máscara de cobertura para el IoU.

## 7. Herramientas del registro

### 7.1 Herramientas nuevas (15)

El orden de registro es: `component_set`, `glyph_compose`, `glyph_decompose`, `glyph_stroke`, `remove_overlap`, `spacing_auto`, `kerning_get`, `kerning_set`, `kerning_auto`, `kerning_check`, `charset_check`, `image_inspect`, `image_trace`, `trace_compare`, `glyph_measure`. Van detrás de las 35 del Hito 1 y siguen el orden en que se construyen.

| Herramienta | Conjunto | Efectos | Qué hace |
|---|---|---|---|
| `image_inspect` | `trace` | lectura | Analiza una imagen dentro de `--allow-dir`. Devuelve la paleta, el color de tinta elegido, las filas (base y altura de mayúsculas en px) y los candidatos (índice, fila, caja). Incluye una **imagen anotada** (PNG sin `structuredContent`) con el número de cada candidato. |
| `image_trace` | `trace` | escritura; destructiva si reemplaza contornos | `assignments: [{candidate, name, unicodes}]` crea o actualiza glifos con el contorno vectorizado, ya limpio y escalado. Devuelve el IoU por glifo. |
| `trace_compare` | `trace` | lectura | IoU por glifo e imagen de superposición para glifos trazados desde una imagen. |
| `component_set` | `components` | escritura; destructiva si reemplaza componentes existentes | Fija los componentes de un glifo en un master. |
| `glyph_compose` | `components` | escritura | Crea un glifo `base + marcas` con colocación automática (`above` / `below` / `center`, `gap` en unidades). El avance es el de la base. |
| `glyph_decompose` | `components` | escritura | Convierte componentes en contornos. |
| `glyph_stroke` | `glyphs` | escritura; destructiva si reemplaza un contorno no vacío | Dibuja un glifo a partir de esqueletos con ancho y tapas, y lo une con `remove_overlap`. Modo `replace` o `append`. |
| `remove_overlap` | `geometry` | escritura | Une los contornos solapados de los glifos indicados. |
| `glyph_measure` | `geometry` | lectura | Mide el estilo de un glifo: grosor de trazo (mediana y percentiles por transformada de distancia), radio de los terminales y estrechamiento en uniones. |
| `spacing_auto` | `spacing` | escritura (masiva si > 20 glifos) | Aplica el espaciado de §6.2 con `tightness`, `references` y `glyphs` opcionales. |
| `kerning_get` | `kerning` | lectura | Devuelve los grupos y los pares de un master, con filtros y límites de respuesta como en el Hito 1. |
| `kerning_set` | `kerning` | escritura | Fija o quita pares y grupos en una transacción. |
| `kerning_auto` | `kerning` | escritura; destructiva si reemplaza kerning existente; masiva si > 20 pares | Calcula los grupos y pares de §6.2. Con `dry_run`, devuelve el plan. |
| `kerning_check` | `kerning` | lectura | Lista las colisiones (pares, distancia mínima), las peores primero. Solo JSON: para verlas, el agente usa `preview_text`. |

La 15.ª herramienta es `charset_check`, en `quality` (lectura). Recibe `profile` (`"unicase-es"`) o una lista explícita `codepoints`, e informa de los códigos faltantes y de qué glifos son compuestos y cuáles tienen contornos. El total pasa de 35 a **50 herramientas**.

- Los conjuntos nuevos `trace`, `components` y `kerning` se suman a los de `--toolsets`.
- Los cambios en el snapshot de `tools/list` son **aditivos**.
- Se actualizan las instrucciones del servidor con el flujo imagen → fuente (§8).

### 7.2 Comandos nuevos en `forma-edit`
`SetComponents`, `Decompose`, `RemoveOverlap` (no destructivo), `SetKerningPair` (valor 0 = quitar), `SetKerningGroup` (crear o cambiar miembros) y `DeleteKerningGroup`. `glyph_compose` se expresa como `CreateGlyph` + `SetComponents` en una sola transacción. `glyph_stroke` e `image_trace` calculan la geometría en el motor y la aplican con el comando de contorno existente (`SetOutline`, destructivo si reemplaza un contorno dibujado).

Todos están disponibles en `edit_batch`.

## 8. Flujo de la demo (instrucciones del servidor)

```
image_inspect → (el agente mira la imagen anotada y asigna A…Z) → image_trace
  → trace_compare → geometry_check/geometry_fix
  → glyph_measure (estilo) → spacing_auto → kerning_auto → kerning_check
  → glyph_compose (Á É Í Ó Ú Ü Ñ con acutecomb/dieresiscomb/tildecomb)
  → glyph_stroke + remove_overlap (cifras, signos, marcas) → preview_text (muestras)
  → charset_check → validate → export_font (TTF + WOFF2) → document_save
```

## 9. Seguridad y límites
- Las imágenes se leen solo dentro de las raíces permitidas (misma sandbox que el resto). Los symlinks que escapan se rechazan.
- Límites:
  - imagen ≤ 64 MiB y ≤ 50 MP;
  - ≤ 500 candidatos por imagen;
  - ≤ 200 asignaciones por `image_trace`;
  - esqueleto ≤ 10 000 nodos;
  - `kerning_check` ≤ 250 000 pares por llamada.
- El contenido de la imagen y los nombres son datos, nunca instrucciones.
- El decodificador de imágenes es un objetivo nuevo del fuzzing nocturno (bytes arbitrarios → error o resultado, nunca pánico, cuelgue ni agotamiento de memoria).

## 10. Pruebas

1. **Unitarias y de propiedades** (proptest, 1 000 casos donde aplique):
   - `fit` respeta el error máximo;
   - un círculo o rectángulo sintético vectorizado da IoU ≥ 0,99;
   - el trazo de un segmento con ancho w tiene grosor w ± 1;
   - `remove_overlap` de dos círculos solapados da el área esperada ± 0,5 % y nunca autointersecciones;
   - componentes: los ciclos se rechazan y descomponer conserva el render;
   - espaciado: un glifo simétrico recibe márgenes simétricos, y repetir el espaciado no cambia nada (idempotente);
   - kerning: nunca produce colisiones, es determinista y sus grupos son consistentes;
   - migración v1 → v2 y round-trip v2 en `.forma` y `.formapackage`.
2. **Imagen de prueba propia** (`tests/fixtures/hito2/specimen.png`, generada por un test a partir de glifos propios: los del Hito 1 más el wordmark FORMA).
   - Dispuesta en dos filas, con fondo oscuro, tinta naranja y una figura distractora azul.
   - El generador está versionado, y la imagen se puede regenerar y comparar.
   - Se conoce el contorno verdadero de cada letra, así que el IoU se mide contra la máscara y contra el contorno.
3. **Ida y vuelta.** Fuente FORMA → PNG → `image_trace` → comparar (criterio 1).
4. **Escenario MCP de punta a punta** con el cliente rmcp, en los dos ciclos de vida: el flujo de §8 sobre la imagen de prueba propia.
   - Termina con `charset_check` al 100 % sobre la lista explícita de códigos de la prueba: las letras de la imagen (A B C F H M O R y sus minúsculas), `space`, Á á Ó ó (compuestas sobre `acutecomb`, dibujada con `glyph_stroke`) y el dígito 0 (dibujado con `glyph_stroke`).
   - `validate` da 0 errores y `kerning_check` 0 colisiones.
5. **Oráculos en CI** (acceptance.sh / .ps1, ampliados):
   - `hb-shape` sobre los pares con kerning devuelve los avances del modelo + kern;
   - fontTools encuentra los compuestos en `glyf` y el kerning en `GPOS`;
   - OTS y fontspector sin FAIL;
   - el WOFF2 se carga en Chrome sin interfaz en Linux (`document.fonts.check`).
6. **Fuzzing nocturno:** nuevo objetivo `image_decode`.
7. **Cobertura:** el gate incluye `forma-trace` y `forma-spacing` (≥ 80 %).

## 11. Documentación
- **ADR 0008:** roadmap "motor primero, hitos por fuente demo, editor al final".
- **ADR 0009:** dependencias `image`, `visioncortex` y `linesweeper` (licencias MIT/Apache, alternativas, plan B Skia PathOps).
- `docs/04-roadmap.md` reescrito con los hitos nuevos:
  - **Hito 2:** de una imagen a una fuente completa.
  - **Hito 3:** fuente original líquida y muestra de dominio público (alternativas, ligaduras, features, anchors, `mark`).
  - **Hito 4:** reconstruir fuentes OFL premium (importación de UFO, designspace y `.glyphs`).
  - **Hitos 5 y siguientes:** variables, CFF, hinting, Python y plugins.
  - **Al final:** interfaz gráfica (spike Qt/Slint, editor) y distribución.
- `docs/07` (catálogo de herramientas), `docs/api/tools.md` (regenerado), `CLAUDE.md` (estado y regla "UI al final"), y la tabla del roadmap del README en EN y ES.
- `docs/qa/hito2/`: demo con la imagen de prueba propia y decisiones del controlador.

## 12. Riesgos

| Riesgo | Mitigación |
|---|---|
| Letras de ~110 px pierden detalle fino | Sobremuestreo ×4, criterio IoU visible por glifo y corrección manual con `nodes_edit` |
| linesweeper es joven (0.5) | Se valida al principio (test de corpus de solapamientos); trait `PathOps` para cambiar a Skia PathOps |
| visioncortex devuelve contornos con nodos de más | Simplificación propia con `fit` + chequeos de geometría |
| La estética de los glifos nuevos es subjetiva | Hoja de prueba aprobada por el responsable (y la autora) |
| `tools/list` crece (49 herramientas) | Conjuntos para cargar solo lo necesario; seguimiento del tamaño en el snapshot |
| Espaciado y kerning automáticos poco "ópticos" | Parámetros expuestos, y `kerning_set` para ajustes finos del agente o la persona |

## 13. Fuera de alcance del Hito 2
- Anchors, `mark`/`mkmk`, ligaduras y alternativas (Hito 3).
- Minúsculas propias (el perfil es unicase).
- Importación de fuentes existentes (Hito 4).
- Multi-master, variables, CFF y hinting (hitos posteriores).
- La interfaz gráfica (al final).
