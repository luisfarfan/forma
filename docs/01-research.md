# 01 — Investigación y análisis competitivo

> Estado: investigación Fase 0, revisión 2 (FORMA, multiplataforma, MCP central) · Fecha: 2026-10-16
> **Convención de evidencia** (obligatoria en todo el documento):
> - **[V]** Verificado en fuente primaria (documentación oficial, repositorio, release notes, appcast, texto legal). URL en §9.
> - **[A]** Afirmación del proveedor (marketing); comportamiento no verificado por nosotros.
> - **[O]** Comportamiento observado por nosotros en la app (pendiente: aún **no** se ha observado ninguna app; ver `05-reverse-engineering.md`).
> - **[H]** Hipótesis o inferencia; no usar como hecho.
>
> Ninguna conclusión de este documento proviene de decompilación ni de análisis de binarios. No se descargó ni instaló software de terceros.

## 1. Resumen ejecutivo

1. **Glyphs 4 existe y es el referente.** 4.0 salió el 2026-07-27; estable actual 4.1.1 (build 4108, 2026-09-12); beta 4.2 (build 4202, 2026-10-14). macOS 12+, €319, licencia perpetua [V].
2. **El formato `.glyphs` está documentado públicamente por su propio fabricante** (especificaciones v2, v3 y v4 + JSON Schemas, en GlyphsSDK, licencia Apache-2.0) [V]. La interoperabilidad con Glyphs **no requiere ingeniería inversa**.
3. **FontLab 8** lleva >2 años sin release mayor (8.4.x); sus usuarios piden señales de continuidad en el foro [V]. **RoboFont 4.6** es Python+AppKit sobre UFO [V]. **Fontra** (GPL-3.0) es el editor libre más activo, web y "variable-first" [V]. **FontForge** sigue vivo pero con cadencia lenta [V].
4. **El ecosistema de compilación cambió en 2026:** fontc 1.0 (Rust, Google) salió el 2026-09-01 — rápido y en paridad alta con fontmake, **pero solo TrueType** [V]. Esto habilita por primera vez construir un editor con un compilador moderno embebido sin depender de Python.
5. **IA:** ningún editor profesional (Glyphs, FontLab, RoboFont) anuncia funciones de IA en sus fuentes oficiales [V para las páginas revisadas]. La investigación académica en generación vectorial de glifos existe pero tiene límites fuertes [V]. Es un espacio abierto, con riesgo de expectativas infladas.
6. **Plataformas:** Glyphs y RoboFont son solo macOS. FontLab (Qt) cubre macOS y Windows, pero no Linux [V]. Fontra y FontForge son multiplataforma y libres (GPL) [V]. **No existe un editor profesional comercial que cubra macOS, Windows y Linux con la misma calidad**, un hueco que FORMA puede ocupar.
7. **MCP:** ningún editor profesional ofrece un servidor MCP oficial. Solo existe un proyecto comunitario para FontLab [A]. Los servidores MCP de herramientas de diseño (Figma, Canva, Blender, Sketch) muestran patrones útiles, pero ninguno documenta control de concurrencia frente a un humano editando [V, ver `07-mcp-ai-architecture.md`].

## 2. Glyphs (Glyphs GmbH, Viena)

### 2.1 Producto y negocio
| Aspecto | Dato | Evidencia |
|---|---|---|
| Empresa | Glyphs GmbH, Viena (Austria); EULA bajo ley austríaca | [V] imprint, EULA |
| Versiones | 4.0 (2026-07-27) · 4.1.1 estable · 4.2 beta; Glyphs 3 (3.5.1) aún mantenido | [V] appcast4/appcast3 |
| Precio | €319 perpetuo; upgrade 199; Mini 2 €49 | [V] /buy |
| Plataforma | Solo macOS, mínimo 12 | [V] /buy, appcast |
| Apple Silicon nativo en G4 | Muy probable | [H] — no hay declaración oficial encontrada |

### 2.2 Arquitectura aparente (solo evidencia pública)
- **App Cocoa/AppKit en Objective-C**, con soporte Swift para plugins. Plantillas Xcode del SDK en `.h/.m` y `.swift` importando `GlyphsCore` [V]. Oferta de empleo 2015: "Cocoa, ObjectiveC… C++ a plus" [V]. Proporción actual ObjC/Swift del núcleo: [H].
- **Tres frameworks públicos:** `GlyphsApp` (tipos para plugins), `GlyphsCore` (núcleo), `GlyphsKit` (extensiones de sistema) [V].
- **Python vía PyObjC:** el módulo `GlyphsApp` envuelve clases ObjC (`objc.lookUpClass("GSFont")`) [V]. El usuario debe instalar Python (G4: 3.13–3.15) desde el Plugin Manager o seleccionarlo [V]. Esto genera fricción de soporte (intérprete equivocado) [V foros].
- **Compilador propio:** desde Glyphs 3 "completely house-made feature compilation, we have dropped Adobe MakeOTF" [V]. En G4 "Use the same variable math as fontTools/VarLib" [V] — indica alineación numérica, **no** que embeba fontTools [H].
- **Hinting:** ttfautohint integrado para TT; AFDKO `otfautohint` para CFF (declaración de staff, 2026-09) [V].
- **Preview de texto con Core Text** (release notes G3) [V].
- **Plugins (GlyphsSDK, Apache-2.0):** bundles `.glyphsFileFormat`, `.glyphsFilter`, `.glyphsPalette`, `.glyphsPlugin`, `.glyphsReporter`, `.glyphsTool`; en Python (con stub compilado) u ObjC/Swift [V]. G4: Plugin Manager sin reinicio, ventana de scripting con breakpoints, Ruff y Pyright [A/V release notes].

### 2.3 Formato de archivo `.glyphs` (documentado por el fabricante)
- OpenStep ASCII plist, UTF-8, gramática BNF; claves ordenadas, números sin comillas, contenedores vacíos omitidos; `.formatVersion` 3 o 4 [V].
- `.glyphspackage`: directorio con `fontinfo.plist`, `order.plist`, `UIState.plist`, `glyphs/*.glyph`; en v4 además `kerning.plist`, `note.md`, `features/*.fea` [V].
- **Nodos:** `[x, y, "tipo"]`. v3: `l` línea, `c` curva, `q` cuadrática, `o` off-curve, sufijo `s` = smooth. **v4 añade** `u` (cuártica), `h` (Hobby), `r` (espiral de Raph Levien), `t` (tangente), orientación, bloqueo y atributos de interpolación de orden superior (HOI) [V].
- **Kerning por master**: `{masterId: {left: {right: valor}}}`, con grupos desde `kernLeft/kernRight/kernTop/kernBottom`; v4 añade `kerningContext` [V].
- **Smart components** (`partsSettings`, `partSelection`, `piece`), **corner/cap/brush/segment/head components** (como `hints` tipados), **brace layers** (`coordinates`) y **bracket layers** (`axisRules`), capas de color [V].
- **Instancias:** `single`, `variable`, `static`, `icon`, `particles` [V].
- **glyphsLib** (Apache-2.0) convierte `.glyphs` → UFO+designspace; soporte del formato v4 no confirmado [H].

**Implicación para FORMA:** la estructura conceptual (font → masters/axes → glyphs → layers → shapes (paths/components) → nodes; kerning por master con grupos; features como texto FEA) es la que el mercado espera. Nuestro modelo debe poder representarla sin pérdida en el subconjunto que soportemos, y **declarar explícitamente lo que no soporta** al importar.

### 2.4 Funcionalidades (resumen; detalle en anexo §8)
- **Dibujo:** pluma, lápiz, primitivas, nodos smooth/corner, cúbicas y cuadráticas; G4: *pen points* (trazos de ancho variable), *star nodes* (transición G2), Fit Curve con balance de manejadores [V/A].
- **Operaciones:** Remove Overlap, Offset Curve, Round Corners, Add Extremes; G4: sustracción, Slanter, Simplify [V/A].
- **Reutilización:** componentes con alineación automática, smart components, corner/cap/segment/brush/head components [V].
- **Espaciado y kerning:** metrics keys (con cálculos), kerning de clases con excepciones; G4: ventana de grupos de kerning, kerning contextual [V/A].
- **OpenType:** sintaxis AFDKO extendida (tokens, condiciones, `#ifdef VARIABLE`); generación automática de features por sufijos de nombre (`.sc`, `.tf`, ligaduras) [V].
- **Variables:** ejes, masters, brace/bracket layers, virtual masters, STAT automático, avar y **avar2** (G4), feature variations, **CFF2** (G4), interpolación de orden superior (G4) [V].
- **Color:** COLRv0/CPAL, COLRv1 (G4 con gradientes/blending), SVG, sbix [V/A].
- **Exportación:** OTF/CFF, TTF, WOFF/WOFF2, VF TTF y CFF2; imágenes PNG/SVG/PDF; SF Symbols; icon sets `.glyphsicons` [V/A].
- **Datos de glifos:** `GlyphData.xml` (nombres, Unicode, categorías, composición); subconjunto público en GlyphsInfo (sin licencia explícita encontrada) [V].

### 2.5 UX (conceptos clave)
- **Font View** (grilla/lista con filtros), **Edit View** (edición de glifos *en contexto de texto*, múltiples pestañas), **Palette** (dimensiones, Fit Curve, capas, transformaciones), **Show All Masters**, herramienta de medición, info box, capas de fondo [V handbook G3].
- G4: barra lateral izquierda (mini font view, formas, sliders de interpolación), selector de master, ventana única Font/Edit, panel de información personalizable [A].
- **Observación clave [H]:** el gran acierto de Glyphs es *editar en contexto de texto*: el diseñador nunca edita un glifo aislado; ve espaciado y kerning mientras dibuja. FORMA debe tener esto desde el MVP, no como añadido.

## 3. FontLab 8

- **Versión 8.4.x**; no hay FontLab 9 anunciado. El director respondió en 2025-11 que siguen trabajando, sin fechas [V]. Usuarios reportan bugs sin respuesta, lentitud de edición en Windows, pérdida de ediciones en 8.2 y documentación de API pobre [V foros].
- **macOS y Windows**; $499 perpetuo; "Starter" $97 por 3 meses [V].
- **Stack:** Qt + PythonQt; usa ttfautohint, AFDKO, fontTools, HarfBuzz, FreeType, PyClipper, etc. (créditos FL7) [V]. Núcleo C++ [H].
- **Scripting:** Python 3.11 embebido (FL 8.2+), paquetes `fontlab`/`fontgate`, sin API formal de plugins comparable a Glyphs [V].
- **Formatos:** VFC (binario), VFJ (JSON, **sin especificación pública**) [V]. Abre/exporta Glyphs y UFO [V].
- **Funciones fuertes:** variables TT y CFF2, hinting visual TrueType, color (SVG, COLRv0/v1, sbix, CBDT), Power Brush/Stroke, Smart Corners, auto-kerning [V].
- **EULA:** prohíbe ingeniería inversa "salvo en la medida permitida por ley imperativa", incluyendo explícitamente observar/estudiar/probar y descompilar para interoperabilidad bajo ley polaca (Directiva 2009/24/CE) [V].

## 4. RoboFont, FontForge, Fontra y otros

| | RoboFont 4.6 | FontForge 20251009 | Fontra (Pak 2026.10.0) |
|---|---|---|---|
| Plataforma | macOS | Linux/macOS/Windows | Navegador + app empaquetada (Mac/Win/Linux) |
| Precio | €400 | Gratis | Gratis |
| Stack | **Python 3.12 + vanilla/AppKit**, defcon, FontParts, `mojo` API [V] | C, toolkit GDraw→GDK/Cairo [V] | Servidor Python + cliente JS por WebSocket [V] |
| Formato nativo | UFO3 [V] | SFD (texto) [V] | designspace/UFO, `.fontra`; lee `.glyphs` (escritura parcial) [V] |
| Compilación | AFDKO embebido (acuerdo con Adobe); VF vía extensión Batch [V] | Propia | fontmake / fontra-compile [V] |
| Extensibilidad | Extensiones (Mechanic 2, Extension Store) [V] | Python [V] | Plugins Python de backend [V] |
| Licencia | Propietaria | GPLv3+ (gran parte BSD) [V] | **GPL-3.0** [V] |

**Lecciones:**
- **RoboFont** demuestra que un editor comercial sobre el ecosistema abierto (UFO + fontTools/AFDKO) es sostenible y que una comunidad técnica valora la extensibilidad [V/H].
- **Fontra** demuestra que la edición variable-first y CJK a gran escala es viable con arquitectura cliente/servidor [V]. Su licencia GPL impide reutilizar código en un producto cerrado; **sí** podemos aprender de su diseño público y documentación [H — ver riesgos legales].
- **FontForge**: amplitud de formatos, pero UI y cadencia son su debilidad percibida [H].

### 4.1 Ecosistema Rust (crítico para la decisión de stack)
- **fontc 1.0.0** (2026-09-01): drop-in de fontmake para Glyphs y designspace/UFO; ~89 % de ~3 450 targets idénticos; **solo TT**, sin instancias estáticas, designspace v4, sin feature writers Python [V].
- **fontations** (read-fonts, write-fonts, skrifa): en producción en Chrome desde Chrome 133 para web fonts [V].
- **norad** (UFO), **kurbo** (Bézier), **HarfRust** (shaping): mantenidos activamente [V].
- **Runebender** (editor Rust de Linebender): inactivo desde 2023, "no apto para uso" [V]. **Lección:** el ecosistema de librerías maduró, pero nadie terminó un editor Rust de nivel profesional; el cuello de botella es el producto/UX, no las librerías [H].

### 4.2 Otros
Glyphr Studio (web, libre, activo) [V]; BirdFont (Vala, libre) [V]; FontStruct (geométrico) [V]; Fontself (plugin Illustrator/Photoshop) [V]; Prototypo (cerrado en 2020, código abierto) [V]. No son competidores directos del segmento profesional.

## 5. IA en diseño tipográfico (estado verificable)

| Fuente | Qué es | Límite relevante |
|---|---|---|
| DeepVecFont-v2 (CVPR 2023) | Generación vectorial de glifos | Limitado a 52 glifos (según VecFusion) [V] |
| DualVector (CVPR 2023) | Representación dual para glifos vectoriales | Investigación [V] |
| VecFusion (CVPR 2024) | Difusión vectorial | Investigación [V] |
| FontDiffuser (AAAI 2024) | Generación *raster* one-shot | Raster, no vectorial [V] |
| Adobe "Project Glyph Ease" (MAX 2023) | Completar set desde pocos glifos | Sneak; sin evidencia de producto lanzado [V] |
| fontlab-mcp-server (comunidad) | Permite a un LLM manejar FontLab | Proyecto comunitario [A] |

Afirmaciones de "auto-kerning con ML" o "trazador IA" en Glyphs/FontLab aparecen solo en listicles SEO y contradicen las páginas oficiales [H — probablemente falsas].

**Oportunidad realista [H]:** IA como *asistente de flujo de trabajo* sobre un modelo de datos sólido (espaciado/kerning sugerido y medible, chequeos de consistencia, generación de código de features, composición de diacríticos, scripting por lenguaje natural vía MCP, acompañamiento de extensión de character set) antes que "generar la fuente". La generación de contornos de calidad profesional no está resuelta por la investigación pública.

## 6. Mercado y dolores de usuarios

- **No existe** encuesta pública con cuotas de uso Glyphs/FontLab/RoboFont [V — búsqueda negativa acotada].
- Anecdótico: Glyphs se recomienda a principiantes por ser autocontenido; RoboFont por flexibilidad; FontLab "menos popular" entre diseñadores de un estudio consultado [V fuentes de foro/blog].
- Dolores verificados: FontLab (estancamiento, bugs, API sin docs) [V]; Glyphs (instalación de Python/PyObjC para plugins) [V]; FontForge (UI) [H].

## 7. Implicaciones para FORMA

1. **Interoperar primero, competir después.** Importar `.glyphs` (spec pública), UFO/designspace y exportar a ellos reduce el costo de cambio de los usuarios. Es requisito comercial, no extra.
2. **Diferenciadores plausibles** frente a Glyphs 4 [H]: (a) compilador moderno y validación integrada (fontspector/OTS en vivo, "esta fuente pasa Google Fonts" como estado visible); (b) Python incluido y API documentada/tipada desde el día 1; (c) IA como asistente con cambios revisables y deshacibles; (d) formato nativo amigable con git (diff por glifo); (e) motor headless/CLI idéntico al de la app para CI de foundries.
3. **No competir en amplitud en v1.** Glyphs tiene ~13 años de features. El MVP debe ser *excelente* en el flujo central: dibujar, espaciar, kernear, exportar una fuente válida.
4. **Riesgo de paridad:** smart components, corner components, HOI, nodos Hobby/espiral, color y hinting manual son trabajo de años. Se priorizan por demanda real (ver roadmap).
5. **Multiplataforma como diferenciador real:** macOS + Windows + Linux con una sola interfaz premium. El precedente de FontLab (Qt) muestra que un editor tipográfico profesional multiplataforma es viable; sus quejas de rendimiento en Windows [V] muestran que la ejecución importa más que el toolkit.
6. **Agentes como usuarios de primera clase:** un servidor MCP oficial sobre el mismo núcleo (sin lógica duplicada), con transacciones, revisiones y aprobaciones, no tiene equivalente en la competencia [V búsqueda acotada]. Detalle en `07-mcp-ai-architecture.md`.

## 8. Anexo — matriz de funcionalidades (resumen)

| Funcionalidad | Glyphs 4 | FontLab 8 | RoboFont 4.6 | Fontra | FORMA (fase objetivo) |
|---|---|---|---|---|---|
| Editor Bézier cúbico | Sí [V] | Sí [V] | Sí [V] | Sí [V] | MVP |
| Edición cuadrática nativa | Sí [V] | Sí [H] | Sí [H] | [H] | Fase 4 |
| Edición en contexto de texto | Sí [V] | Sí [H] | Parcial (extensiones) [H] | Sí [H] | MVP |
| Plataformas | macOS [V] | macOS, Windows [V] | macOS [V] | Navegador / Mac / Win / Linux [V] | **macOS, Windows, Linux** |
| Servidor MCP oficial | No [V búsqueda] | No (solo comunitario) [A] | No [H] | No [H] | **Hito 1** |
| Componentes + anclas | Sí [V] | Sí [V] | Sí [V] | Sí [H] | Fase 2 |
| Smart components / variables locales | Sí [V] | Variable components [H] | Extensiones [H] | Sí (variable components) [H] | Fase 6 |
| Kerning de clases + excepciones | Sí [V] | Sí [V] | Sí [V] | Sí [H] | Fase 2–3 |
| Multi-master / VF TT | Sí [V] | Sí [V] | Vía Batch [V] | Sí [V] | Fase 3 |
| CFF / CFF2 | Sí [V] | Sí [V] | CFF sí; VF vía Batch [V] | Vía fontmake [V] | Fase 4 / 6 |
| avar2 | Sí [V] | [H] | [H] | [H] | Fase 6 |
| Features FEA + automáticas | Sí [V] | Sí [V] | Sí [H] | [H] | Fase 4 |
| Color COLRv1 | Sí [V/A] | Sí [V] | [H] | Parcial [H] | Fase 7 |
| Hinting auto TT | ttfautohint [V] | ttfautohint [V] | ttfautohint (externo) [V] | — | Fase 6 |
| Scripting Python | Sí (Python instalado por usuario) [V] | Sí (embebido) [V] | Sí (nativo) [V] | Plugins backend [V] | Fase 5 |
| Validación integrada (Font Bakery/fontspector) | No encontrada [H] | Parcial (avisos) [V] | Extensiones [H] | — | Hito 1 (diferenciador) |
| IA | No anunciada [V] | No anunciada [V] | No [H] | No [H] | Hito 1 (vía MCP) · Fase 2 (agente integrado) |

## 9. Fuentes

**Glyphs:** https://glyphsapp.com/ · https://glyphsapp.com/buy · https://glyphsapp.com/buy/upgrade · https://glyphsapp.com/buy/eula · https://glyphsapp.com/contact · https://glyphsapp.com/news/glyphs-4-create-love-the-process · https://updates.glyphsapp.com/appcast4.xml · https://updates.glyphsapp.com/appcast3.xml · https://updates.glyphsapp.com/Glyphs4.1-4100.html · https://updates.glyphsapp.com/Glyphs4.2-4202.html · https://updates.glyphsapp.com/Glyphs3.5.1-3535.html · https://docu.glyphsapp.com/ · https://docu.glyphsapp.com/Core/index.html · Handbook G3 (archivado): https://web.archive.org/web/20250707081028/https://handbook.glyphsapp.com/single-page/ · GlyphsSDK: https://github.com/schriftgestalt/GlyphsSDK · Formato v3: https://github.com/schriftgestalt/GlyphsSDK/blob/Glyphs3/GlyphsFileFormat/GlyphsFileFormatv3.md · Carpeta de formato (v2/v3/v4 + schemas): https://github.com/schriftgestalt/GlyphsSDK/tree/Glyphs3/GlyphsFileFormat · Wrapper Python: https://github.com/schriftgestalt/GlyphsSDK/blob/Glyphs3/ObjectWrapper/GlyphsApp/__init__.py · GlyphsInfo: https://github.com/schriftgestalt/GlyphsInfo · glyphsLib: https://github.com/googlefonts/glyphsLib · Foros: https://forum.glyphsapp.com/t/glyphs-4-release-date/25406 · https://forum.glyphsapp.com/t/autohinting-for-exporting-otf/37181 · https://forum.glyphsapp.com/t/using-a-different-feature-files-compiler/12981 · https://forum.glyphsapp.com/t/apple-silicon-support/17832 · https://forum.glyphsapp.com/t/we-are-looking-to-hire-a-mac-developer/3470 · https://forum.glyphsapp.com/t/glyphs-app-python-installation-and-pyobjc/23314

**FontLab:** https://www.fontlab.com/font-editor/fontlab/ · https://fontlab.com/eula · https://help.fontlab.com/fontlab/8/whats-new/whats-new-12-scripts-extensions · https://help.fontlab.com/fontlab/7/manual/FontLab-Credits/ · https://help.fontlab.com/fontlab-vi/Font-Formats/ · https://forum.fontlab.com/fontlab-8/will-fontlab-continue-to-be-developed/ · https://forum.fontlab.com/fontlab-8/will-fontlab-9-have-full-vfcvfj-interoperability/ · https://forum.fontlab.com/fontlab-8/bug-extremely-slow-response-when-drawing/

**RoboFont / FontForge / Fontra:** https://robofont.com/ · https://www.robofont.com/licensing · https://robofont.com/documentation/topics/robofont-history/ · https://www.robofont.com/documentation/tutorials/generating-fonts · https://github.com/fontforge/fontforge/releases · https://raw.githubusercontent.com/fontforge/fontforge/master/LICENSE · https://github.com/fontra/fontra · https://docs.fontra.xyz/ · https://fontra.xyz/

**Rust:** https://github.com/googlefonts/fontc/releases/tag/fontc-v1.0.0 · https://github.com/googlefonts/fontations · https://developer.chrome.com/blog/memory-safety-fonts · https://github.com/linebender/runebender · https://github.com/linebender/norad

**Otros / IA / mercado:** https://www.glyphrstudio.com/ · https://birdfont.org/ · https://fontstruct.com/ · https://www.fontself.com/ · https://arxiv.org/abs/2303.14585 · https://arxiv.org/abs/2305.10462 · https://arxiv.org/abs/2312.10540 · https://arxiv.org/abs/2312.12142 · https://blog.adobe.com/en/publish/2023/10/11/max-sneaks-2023 · https://github.com/dithilli/fontlab-mcp-server · https://typedrawers.com/discussion/3070/robofont-or-glyphs-for-type-design · https://blog.arrowtype.com/robofont-vs-glyphs/
