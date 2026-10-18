# ADR 0008: Roadmap "motor primero": hitos guiados por fuentes demo, interfaz gráfica al final y ninguna fuente comercial

- Estado: aceptada (Hito 2)
- Fecha: 2026-10-17
- Referencias: `docs/04-roadmap.md` (revisión 4), `docs/superpowers/specs/2026-10-17-hito2-imagen-a-fuente-design.md` §1, `docs/06-risks.md`, `CLAUDE.md`

> Este ADR ocupa el número 0008 que `docs/03-architecture.md` §15 reservaba para el toolkit de UI. La decisión del toolkit se registrará con el número que corresponda cuando se haga el spike de UI (Hito 8 del roadmap nuevo).

## Contexto

El Hito 1 dejó un núcleo sin interfaz, con CLI y servidor MCP, verificado en macOS, Windows y Ubuntu (CI en verde, fuzzing nocturno en verde). El roadmap de la revisión 3 seguía con el spike comparativo de UI (Hito 2), el editor mínimo (Hito 3), la distribución (Hito 4) y después fases por capacidades: componentes y kerning, multi-master, OpenType y formatos de producción, interoperabilidad.

El 2026-10-17 el responsable redirigió el rumbo con tres decisiones:

1. **Motor primero, editor gráfico al final.** Todo lo que hace falta para crear una fuente perfecta (kerning, componentes, variables, features OpenType, CFF, hinting, importación) va antes que la interfaz.
2. **Hitos guiados por fuentes demo.** Cada hito se define por una fuente concreta que FORMA debe poder crear, y se construye lo que esa fuente exige.
3. **Nada de fuentes comerciales.** Las demostraciones usan dibujos con derechos claros, muestras de dominio público y fuentes OFL.

La primera fuente demo es el alfabeto A–Z que dibujó la hermana del responsable. Es la única imagen disponible: se queda en local y no entra en el repositorio.

Razones que apoyan el cambio [H, no medidas]:
- El motor ya se maneja entero por MCP y CLI, así que se puede ejercitar de punta a punta sin interfaz.
- Una interfaz construida antes de que el modelo esté completo habría que rehacerla cada vez que el modelo crece: componentes, kerning y masters cambian qué datos existen y qué debe mostrar el editor.
- La incertidumbre mayor (Qt frente a Slint, `docs/06-risks.md` T14) es también la que menos evidencia tiene; decidirla con un motor completo da un spike más representativo.

## Decisión

1. **Motor primero.** Los Hitos 2 a 7 son headless (núcleo, CLI y MCP). La interfaz gráfica, con su spike de toolkit y el editor, es el Hito 8, y la distribución con la v1.0 es el Hito 9.
   - La definición de "terminado" exige UI **desde el Hito 8** y no desde el Hito 3.
   - La regla 1 de arquitectura no cambia: la UI será otra fachada del registro de comandos.
   - Al llegar al Hito 8, la UI de todo lo construido antes (Hitos 2–7) forma parte de su alcance.
   - Hasta aprobar el Hito 8 siguen sin construirse el editor gráfico ni instalarse Qt o Slint. `forma-canvas` conserva su contrato independiente del toolkit.
2. **Hitos guiados por fuentes demo.** Cada hito lleva una fuente demo, criterios de aceptación automatizados en CI y, como mucho, una demo manual. El alcance sale de lo que la fuente exige.

   | Hito | Qué hace FORMA al cerrarlo |
   |---|---|
   | 2 | Imagen → fuente completa (el alfabeto de la hermana, juego `unicase-es`) |
   | 3 | Fuente original "líquida" + muestra de dominio público (anchors, `mark`/`mkmk`, alternativas, ligaduras, features) |
   | 4 | Reconstrucción de fuentes OFL premium (importación de UFO, designspace y `.glyphs`; comparación con el binario oficial) |
   | 5 | Multi-master y fuentes variables |
   | 6 | CFF/OTF y hinting |
   | 7 | Python y plugins |
   | 8 | Spike de UI y editor |
   | 9 | Distribución y v1.0 |

   Solo el Hito 2 tiene especificación detallada; los criterios de los demás son provisionales hasta que cada hito tenga la suya.
3. **Sin fuentes comerciales, ni siquiera en local.**
   - No se usan como fixtures, ni como entrada de pruebas locales, ni se reconstruyen o importan para comparar. Solo sirven como referencia visual (mirarlas).
   - Se permiten dibujos con derechos claros, muestras de dominio público (con procedencia documentada en `tests/fixtures/<nombre>/`) y fuentes OFL. El Hito 4 reconstruye fuentes OFL, no comerciales.
   - **Las imágenes de terceros nunca se versionan.** El alfabeto de la hermana y la fuente resultante se quedan en local (`~/fonts/hermana/`). Solo se publican si la autora lo autoriza, con su nombre y la licencia que elija. Los tests usan una imagen propia generada por código.
4. **Numeración.** Este ADR renumera los hitos. `docs/04-roadmap.md` incluye la tabla de equivalencias con la numeración de la revisión 3 (que usan todavía `docs/01`, `02`, `03`, `07` y los ADR 0004 y 0006).

## Alternativas

- **B. Hitos por capacidades** (componentes, luego kerning, luego variables…, sin fuente concreta): cada hito se cerraría con pruebas sintéticas y no se sabría qué parte exige realmente el trabajo de un diseñador. Se corre el riesgo de dejar capacidades a medias, sin un uso que las pruebe. Descartada.
- **C. Script aparte** que vectorice la imagen y arme la fuente fuera del motor: sería rápido para la demo, pero rompe la regla de "una sola lógica" (regla 1), duplica el modelo, y no produciría un documento editable, con historial y deshacer, ni ejercitaría MCP ni CLI. No deja capacidades reutilizables para el hito siguiente. Descartada.
- **Mantener el spike de UI como Hito 2** (roadmap de la revisión 3): retrasa lo que el responsable quiere ver primero, una fuente completa, y obliga a decidir el toolkit con un modelo incompleto. Descartada.
- **Usar fuentes comerciales solo en local**, sin versionarlas: dejaría en cada máquina datos con licencia de uso restrictiva y haría la regla más difícil de auditar. Una regla sin excepciones se verifica con una búsqueda. Además refuerza la sala limpia (`docs/05-reverse-engineering.md`) y baja los riesgos L1 y L5. Descartada.

## Consecuencias

- Hasta el Hito 8 no hay interfaz. Toda demostración es por CLI o por un agente vía MCP, y la verificación visual usa las vistas previas PNG/SVG del servidor.
- La deuda de UI se acumula: el Hito 8 tendrá que dar interfaz a todos los comandos de los Hitos 2–7, por lo que será uno de los hitos más grandes (XL, estimación [H]). A cambio, cada comando llega a la UI con schema, pruebas, concurrencia por entidad y deshacer ya resueltos.
- Riesgo de descubrir tarde que el modelo no se presta a la interacción (gestos, rendimiento con miles de nodos). Mitigación: `forma-canvas` y el registro se mantienen independientes del toolkit, y el presupuesto de rendimiento se mide en el spike. Los riesgos T4 y T14 pasan al Hito 8.
- Las fuentes demo exigen fixtures propias o con procedencia clara, y el repositorio no incluirá imágenes de terceros. El criterio 8 del Hito 2 (la demo con la hermana) es manual y local, así que no bloquea la CI.
- Los criterios de los Hitos 3–9 son provisionales [H]: este ADR fija el orden y la fuente demo, no los detalles. Cada hito tendrá su especificación antes de empezar.
- Documentos que usan la numeración anterior (`docs/01`, `02`, `03`, `07` y los ADR 0004 y 0006) se actualizan cuando se toquen; mientras tanto vale la tabla de equivalencias del roadmap. `docs/03-architecture.md` §15 sigue reservando el 0008 para el toolkit y debe corregirse.
