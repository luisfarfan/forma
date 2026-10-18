# 06 — Riesgos técnicos, legales y de producto

> Estado: revisión 4 (open source MIT OR Apache-2.0; motor primero, ADR 0008) · 2026-10-17 · Revisar al cierre de cada hito. Los hitos se numeran como en la revisión 4 de `04-roadmap.md`.
> Probabilidad (P) e impacto (I): **A** alta · **M** media · **B** baja. ★ = condicionan decisiones actuales.

## 1. Riesgos técnicos

| ID | Riesgo | P | I | Mitigación | Señal de alerta |
|---|---|---|---|---|---|
| T1 ★ | **fontc 1.0 solo genera TrueType** (sin CFF/CFF2, sin instancias estáticas, sin remove overlap en estáticas) [V] | A | A | TTF/VF-TTF hasta el Hito 5; instancias estáticas propias (Hito 5); CFF con sidecar fontmake (Hito 6); seguir el trabajo upstream | Clientes beta exigen OTF antes del Hito 6 |
| T2 | API de fontc como librería poco estable ("should be referenced only by fontc") [V] | M | M | Solo la API pública con designspace+UFO intermedios; versiones fijadas; plan B: binario `fontc` | Roturas al actualizar |
| T3 ★ | **Pérdida o corrupción de datos** (serialización, caída durante el guardado, migraciones, SQLite en carpetas de nube o de red) | M | A | Transacciones SQLite sin WAL; escritura atómica en `.formapackage`; journal; round-trip con propiedades; fuzzing; tests de kill en 3 SO; guardado con copia y reemplazo en ubicaciones sincronizadas | Cualquier pérdida de datos = incidente P0 |
| T4 | Rendimiento del puente núcleo↔UI y de los gestos | M | M | API gruesa; escena y herramientas en Rust; medición en el spike | Fuera de presupuesto en el spike (Hito 8) |
| T5 | Booleanas poco robustas en casos degenerados [V: no hay librería Rust madura] | A | M | `linesweeper` detrás del trait `PathOps` y Skia PathOps como plan B (ADR 0009); corpus de casos degenerados (ver T19) | Fallos del corpus |
| T6 | Errores en la matemática de variaciones | M | A | Pruebas diferenciales contra fontTools; checker estricto | Diferencias fuera de tolerancia |
| T7 | Complejidad de varios lenguajes (Rust + QML + C++ mínimo con Qt) | M | M | C++ limitado a la capa cxx-qt; lógica en Rust; CI desde el día 1 | Fricción frecuente en bindings |
| T8 | Rotación de dependencias (fontations pre-1.0, rmcp con releases semanales, Qt/Slint) [V] | A | B | `Cargo.lock`; actualizaciones deliberadas con suite diferencial; `cargo-deny` | Roturas al actualizar |
| T9 | Python embebido multiplataforma (tamaño, firma, notarización, validación de librerías) | A | M | Diferido a los Hitos 6–7; prototipo de empaquetado previo | Notarización o firma fallidas |
| T10 | Evolución del formato `.glyphs` (v4 publicado el 2026-07-17) [V] | M | M | Importador desde la especificación y los JSON Schemas oficiales; informe de pérdidas | Nueva versión de la especificación |
| T11 | Rendimiento a escala (CJK) | M | M | Escritura incremental; caché por revisión; benchmarks desde el Hito 8 (spike) | Abrir 3 000 glifos > 1 s |
| T12 ★ | **Seguridad**: archivos no confiables, plugins, contenido devuelto por LLMs, **prompt injection a agentes desde el contenido de fuentes** | M | A | Parsers en Rust con fuzzing; plugins aislados; contenido como datos; aprobaciones aplicadas por el servidor; sin ejecución de código vía MCP en v1 | Hallazgos del fuzzer o de pruebas de inyección |
| T13 | Discrepancias de precisión entre editor y binario | M | M | Redondeo explícito; preview "real" desde compilación en memoria; nada de renderers que conviertan cúbicas en cuadráticas | Reportes de "se ve distinto" |
| T14 ★ | **Toolkit de UI**: Qt (licencia y costo; cxx-qt en "early development" [V]); Slint (sin presión de lápiz, interop wgpu inestable [V]); ninguno tiene evidencia pública de 120 Hz [V búsqueda] | M | A | Canvas y herramientas en Rust, independientes del toolkit; spike con medidas (Hito 8); decisión de licencia antes del código de producto | Spike fuera de presupuesto en algún SO |
| T15 ★ | **Diferencias entre SO** en entrada (lápiz, IME, gestos), rutas, bloqueos de archivo, IPC, fuentes del sistema, firma y empaquetado | A | M | Capa `forma-platform` delgada; CI en 3 SO en cada PR; pruebas manuales por SO en cada hito; hardware de referencia por SO | Fallos solo en un SO |
| T16 ★ | **Fragmentación de MCP**: dos eras incompatibles del protocolo (Codex en 2025-06-18; especificación vigente 2026-07-28) [V]; soporte desigual de elicitation, resources y Tasks; Codex descarta imágenes si hay `structuredContent` [V]; límites de salida y de número de herramientas [V] | A | M | rmcp (ambas eras); diseño por mínimo común denominador; tests en ambos modos; previews sin `structuredContent`; toolsets; snapshot de contrato | Clientes nuevos con comportamientos distintos |
| T17 | **Agentes que degradan la calidad** (cambios masivos plausibles pero malos; bucles de reintento) | M | M | Planes para operaciones masivas; validación en cada export; historial atribuido con revert fácil; evals con validadores | Quejas de usuarios sobre cambios de agentes |
| T18 | Concurrencia app ↔ `forma mcp` (bloqueos, proxy IPC, documentos abiertos dos veces) | M | A | Escritor único por documento; bloqueo de archivo; solo lectura si no hay IPC; tests de concurrencia | Corrupción o pérdida de cambios en pruebas |
| T19 ★ | **`linesweeper` es joven** (0.5, repositorio de un solo usuario, unas 45 estrellas [V 2026-10-17]) y `remove_overlap` depende de él para unir trazos y contornos trazados | M | M | Corpus de solapamientos validado al empezar el Hito 2; trait `PathOps` en `forma-geometry` para cambiar a Skia PathOps (plan B, ADR 0009); pruebas de propiedades (área ±0,5 %, sin autointersecciones); `INVALID_GEOMETRY` en lugar de contornos corruptos; versión fijada en `Cargo.lock` | Fallos del corpus o autointersecciones tras `remove_overlap` |
| T20 ★ | **Fidelidad del trazado a baja resolución:** con letras de ~110 px se pierde detalle fino (terminales, uniones estrechas) y `visioncortex` devuelve nodos de más [H] | A | M | Máscara sobremuestreada ×4 (precisión subpíxel); IoU por glifo con umbral 0,97 y superposición en PNG (`trace_compare`); simplificación propia (`fit`) y chequeos de geometría; corrección manual con `nodes_edit`; una imagen de mayor resolución si la autora la tiene | IoU < 0,97 en letras reales; hoja de prueba rechazada |

## 2. Riesgos legales

| ID | Riesgo | P | I | Mitigación |
|---|---|---|---|---|
| L1 ★ | Reclamación por ingeniería inversa de Glyphs (EULA bajo ley austríaca; §2 "alter, translate") [V] | B (con política) | A | Política de niveles (`05-reverse-engineering.md`): solo fuentes públicas y caja negra; opinión legal antes del nivel ≥ 2 |
| L2 | Similitud de UI, trade dress o marca de Glyphs | M | M | Diseño e iconografía originales; uso nominativo de "Glyphs" |
| L3 ★ | **Compatibilidad de licencias con MIT OR Apache-2.0:** no se puede incorporar código GPL (Fontra, FontForge [V]) ni módulos de Qt solo GPL/comerciales (Canvas Painter, Graphs, Charts, Quick 3D, `qmlsc`) [V]; Qt LGPLv3 exige enlace dinámico, avisos y acceso al fuente de Qt [V]; Slint royalty-free exige atribución [V]; FTL exige crédito | M | A | `cargo-deny` con lista de licencias permitidas; SBOM por release; lista de módulos de Qt permitidos en CI; archivo de avisos de terceros (`THIRD_PARTY_NOTICES`) |
| L4 | Datos sin licencia clara (`GlyphData.xml`, GlyphsInfo) [V] | M | M | Base de datos propia desde Unicode UCD y AGLFN |
| L5 | Fuentes de prueba y datos de entrenamiento de IA; derechos sobre contornos generados | M | A | Solo fixtures propias u OFL; no entrenar con fuentes sin licencia; términos de uso claros |
| L6 ★ | **Privacidad y confidencialidad:** diseños no publicados enviados a proveedores LLM (agente integrado); RGPD | M | A | IA opt-in; visibilidad de lo enviado; BYOK; modelos locales; MCP local por stdio (FORMA no envía nada: el cliente MCP decide) |
| L7 ★ | **Marca "FORMA":** **Forma DJR**, familia tipográfica comercial muy conocida de David Jonathan Ross (revival de Forma, Nebiolo 1968), vendida al **mismo público** (diseñadores, tipógrafos) [V]. **Autodesk Forma**, marca en expansión con solicitudes en EE. UU. en las **clases 9 y 42** [V snippets USPTO]. Otras: marca "FORMA" para software de documentos (USPTO 98578915), Unity Forma [V snippets]. No se pudieron consultar USPTO, EUIPO ni WIPO de forma automatizada [V] | **M** | **M** | Para un proyecto open source sin venta el riesgo baja, pero no desaparece: un nombre que coincide con una tipografía muy conocida genera confusión en la comunidad, y los titulares pueden objetar el uso. Hacer al menos una búsqueda básica (USPTO, EUIPO/TMview, WIPO, INDECOPI) antes de anunciar el proyecto; considerar una variante distintiva si la comunidad lo percibe como conflicto. El nombre en código `forma-*` se mantiene |
| L8 | Extensión `.forma` usada por proyectos pequeños (un editor vectorial web, un lenguaje de programación) [V] | M | B | Firma de contenido (`application_id` de SQLite y magic en el manifiesto); registro del tipo en los 3 SO |
| L9 | Licencias de herramientas de ingeniería inversa propietarias (Hopper, IDA, JEB) | B | B | Irrelevante mientras la política prohíba el nivel 3 |

## 3. Riesgos de producto y negocio

| ID | Riesgo | P | I | Mitigación |
|---|---|---|---|---|
| P1 ★ | Paridad con Glyphs 4 inalcanzable a corto plazo [V] | A | A | Ser excelente en el flujo central y diferenciarse en: 3 SO, MCP y agentes, validación integrada, formato apto para git, CLI/CI |
| P2 ★ | Costo de cambio para usuarios de Glyphs | A | A | Interoperabilidad UFO y `.glyphs` (Hito 4) de alta fidelidad; adelantarla si la beta lo exige |
| P3 | Escepticismo de la comunidad hacia la IA [H] | M | M | IA opcional, transparente y deshacible; FORMA es completo sin IA; MCP como herramienta para usuarios técnicos |
| P4 | **Costo de soportar 3 SO** (QA, soporte, hardware, firma) | A | M | CI en 3 SO desde el Hito 1; canvas e interacción en Rust; matriz de soporte explícita (Ubuntu 24.04+ y Flatpak para otras distros) |
| P5 | **Sostenibilidad del proyecto open source** (sin ingresos: tiempo de mantenimiento, costos de firma de código en macOS y Windows, CI de 3 SO) | A | A | Mantener el alcance acotado; CI en runners gratuitos para open source; buscar patrocinio (GitHub Sponsors, Open Collective, subvenciones para tipografía y software libre); atraer contribuidores con buena documentación |
| P6 | Equipo pequeño / factor bus; dependencia de agentes para implementar | A | A | ADRs, CI estricta, criterios de aceptación automatizados, `CLAUDE.md`; contribuidores externos (CONTRIBUTING, issues etiquetados) |
| P9 | **Gobernanza open source**: contribuciones de calidad variable, PRs con código de procedencia dudosa (copiado de software propietario o GPL) | M | M | CONTRIBUTING con política de procedencia y DCO (`Signed-off-by`); revisión obligatoria; la política de ingeniería inversa aplica también a contribuidores |
| P7 | Pérdida de confianza por bugs graves | M | A | Validación integrada; beta cerrada; incidentes P0 |
| P8 | Alcance del MVP (ahora con MCP y 3 SO) demasiado grande | M | M | Hitos pequeños con cierre verificable; el Hito 1 es headless; recortar funcionalidades antes que tests |
| P10 | **Estética subjetiva de los glifos generados:** cifras, signos y acentos nuevos en el estilo de la autora pueden ser técnicamente correctos y verse mal; no hay una métrica objetiva de "gusto" [H] | A | M | Hoja de prueba (`preview_text`) revisada y aprobada por el responsable y la autora (criterio 8 del Hito 2); medición del estilo con `glyph_measure` (grosor, terminales, estrechamiento) para igualar el dibujo original; parámetros expuestos y retoque manual con `nodes_edit`; publicar la fuente es opcional y requiere la autorización de la autora |

## 4. Decisiones discutibles y alternativas

| Decisión propuesta | Por qué es discutible | Alternativa |
|---|---|---|
| Qt 6.12 + cxx-qt como UI | Obligaciones LGPL, sin parches LTS para open source, tres lenguajes, cxx-qt inmaduro | Slint (100 % Rust, sin presión de lápiz); GPUI (pre-1.0); Tauri 3 con CEF cuando sea estable |
| Canvas e interacción en Rust, no en el toolkit | Más trabajo inicial | Usar las primitivas de dibujo del toolkit (más rápido de hacer, más dependencia del toolkit) |
| `.forma` en SQLite + `.formapackage` | Dos contenedores | Solo carpeta (problemas en Windows/Linux) o zip (reescritura completa) |
| MCP con dos eras del protocolo | Más superficie de pruebas | Solo 2026-07-28 (rompe Codex hoy) [V] |
| Concurrencia optimista por entidad | Más complejidad que un bloqueo | Bloqueo exclusivo agente/usuario (peor experiencia de colaboración) |
| fontc como compilador principal | Sin CFF hasta el Hito 6 | fontmake desde el inicio (Python embebido desde el Hito 1) |
| Agente integrado en el Hito 8, con la UI (ADR 0008) | Diferenciador tardío dentro de la app | Adelantar un agente sin interfaz sobre `forma-agent` (el registro ya lo permite; no cuesta UI) |
| Nombre FORMA | Conflictos de marca (L7) | Marca compuesta o alternativa tras la búsqueda legal |
