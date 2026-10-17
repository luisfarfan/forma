# ADR 0006: Compilación con fontc; validación propia dentro del producto y OTS, fontspector y HarfBuzz como oráculos de CI en el Hito 1

- Estado: aceptada (Hito 1)
- Fecha: 2026-10-16
- Referencias: `docs/02-tech-stack.md` §8, `docs/03-architecture.md` §10 y §13, `docs/04-roadmap.md` (Hito 1), `docs/qa/hito1/`

> La tabla de `docs/03-architecture.md` §15 reservaba el 0006 para la compilación con fontc, y el roadmap cita el 0006 para la decisión de la validación. Este ADR recoge las dos, que son el mismo problema: cómo se produce una fuente y cómo se sabe que es válida.

## Contexto

Una fuente exportada que un navegador o un sistema operativo rechaza es un fallo grave y silencioso. Hacía falta un compilador que se pueda llevar dentro del producto, sin Python, y una forma de comprobar su salida con herramientas independientes. Al mismo tiempo, integrar dentro del producto verificadores escritos en C++ o en otros lenguajes añade dependencias pesadas en los tres sistemas.

## Decisión

**Compilación.**
- `forma-compile` usa **fontc 1.0 como biblioteca**, dentro del proceso. El *preflight* trabaja sobre una copia: comprueba rangos de OpenType y geometría, añade `.notdef` (un rectángulo hueco, siempre en el glifo 0) y `space` si faltan, y avisa de lo que añadió. Después escribe el master por defecto como **UFO** en una carpeta temporal privada y compila ese UFO con fontc. Con un solo master no hace falta designspace; llegará con los varios masters (Hito 5; entonces fase 3), que es el diseño "designspace + UFO intermedios".
- WOFF2 se comprime con `ttf2woff2` (Rust puro). La primera versión usaba `woofwoof`, cuyo `build.rs` compila C++ con opciones solo de GCC y rompería MSVC; se descartó para cumplir la regla de los tres sistemas.
- **fontmake es el oráculo**, no una dependencia del producto: la CI compila el mismo UFO con fontmake y exige el mismo conjunto de glifos y `cmap`, el mismo ancho y número de contornos por glifo, cajas a ±1 unidad y área a ±0,5 %. Las demás diferencias (puntos implícitos, orden de contornos, registros de `name`) se listan y se explican, no fallan (`docs/qa/hito1/fontmake-parity.md`).

**Validación.**
- Dentro del producto, `forma-validate` hace **chequeos propios**: del modelo (geometría de los contornos exportados, código Unicode ausente, métricas verticales) y del binario compilado leído con `read-fonts` (tablas obligatorias, `cmap`, `hmtx`, `.notdef`, nombres en `post`). El informe tiene `errors`, `warnings` y `compiled`, e `invariants` o `compile` fallidos detienen los chequeos siguientes.
- **OTS, fontspector y HarfBuzz se ejecutan solo como verificación externa en CI**, en un venv con versiones fijadas (`tools/oracle/requirements.txt`: fontTools 4.67.0, fontmake 3.12.2, opentype-sanitizer 9.2.0, uharfbuzz 0.56.3) y `fontspector` 1.9.0 con el perfil `opentype`. `scripts/acceptance.sh` y `scripts/acceptance.ps1` construyen la fuente de demostración "Forma Demo" con un agente por MCP y la pasan por OTS (TTF y WOFF2), el shaping de HarfBuzz, fontTools y la paridad con fontmake, en los tres sistemas; fontspector corre en Ubuntu, donde es obligatorio (`FORMA_REQUIRE_FONTSPECTOR=1`), y debe dar 0 FAIL (los WARN se justifican en `docs/qa/hito1/fontspector.md`).
- Integrar OTS o fontspector en el producto se reevalúa en un hito posterior (la numeración original decía «fase 2»; ver la tabla de equivalencias de `docs/04-roadmap.md`).

## Alternativas

- **fontmake como única ruta:** exigiría Python dentro del producto y de la CLI; se conserva como oráculo y como ruta para CFF (Hito 6).
- **Invocar el binario `fontc`:** el plan B si la API de biblioteca fallaba; no hizo falta.
- **OTS dentro del producto:** es C++ (builds distintos por sistema) y el valor de validar en cada exportación frente a validar en CI no justifica la dependencia en el Hito 1.
- **fontspector dentro del producto:** hay crates disponibles, pero el perfil de comprobaciones aún no está acordado y añade una dependencia pesada.
- **Depender solo de los oráculos de CI:** `validate` es una herramienta de agente; sin chequeos propios, un agente no podría cerrar el ciclo editar → validar → corregir sin salir del producto.

## Consecuencias

- El producto no garantiza lo que OTS o fontspector garantizan; la garantía la da la CI sobre la fuente de demostración, no sobre cualquier fuente de una persona.
- fontc solo produce TrueType: CFF llega en el Hito 6, las fuentes estáticas desde variables en el Hito 5 y CFF2 después de v1.0.
- La paridad con fontmake se sostiene en parte porque la fuente de prueba no tiene contornos superpuestos (fontmake los fusiona por defecto y fontc no); al permitir y quitar solapamientos habrá que compilar con `--keep-overlaps` o quitarlos antes.
- fontc no expone avisos por su API de biblioteca, así que `CompileReport.warnings` está vacío hoy.
- Los WOFF2 pueden diferir en tamaño de los del codificador de Google; OTS los acepta.
- Se necesita Python 3.12 en la CI para los oráculos; los usuarios no lo necesitan.
