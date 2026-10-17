# ADR 0009: Dependencias de trazado y booleanas: `image`, `visioncortex` y `linesweeper`, con Skia PathOps como plan B detrás del trait `PathOps`

- Estado: aceptada (Hito 2)
- Fecha: 2026-10-17
- Referencias: `docs/superpowers/specs/2026-10-17-hito2-imagen-a-fuente-design.md` §6.1, §6.3, §9 y §12; `docs/06-risks.md` (T5, T19, T20); `CLAUDE.md` (regla de dependencias)

## Contexto

El Hito 2 convierte la imagen de un alfabeto en contornos de fuente. Eso exige tres capacidades que el núcleo no tenía:
1. **Leer imágenes** PNG y JPEG de forma segura (el archivo es una entrada no confiable).
2. **Vectorizar** una máscara binaria de cada letra en curvas.
3. **Unir contornos solapados** con relleno no-cero (`remove_overlap`). Lo necesitan el trazo de esqueleto (`glyph_stroke`) y la limpieza de los glifos trazados.

Las restricciones vienen de `CLAUDE.md`: licencia compatible con `MIT OR Apache-2.0` (el código GPL está prohibido), sin dependencias de SO en el núcleo, y compilación y pruebas en macOS, Windows y Ubuntu 24.04. En el registro de riesgos (T5) consta que no hay una librería Rust madura de booleanas [V].

## Decisión

Se incorporan tres dependencias directas. Los datos de versión, licencia y actividad se verificaron el 2026-10-17 [V].

| Crate | Versión | Licencia | Para qué sirve | Mantenimiento |
|---|---|---|---|---|
| `image` | 0.25.10 | MIT OR Apache-2.0 | Decodificar PNG y JPEG. | Crate de referencia del ecosistema (image-rs). |
| `visioncortex` | 0.9.3 | MIT OR Apache-2.0 | Vectorizar la máscara binaria de cada candidato en curvas. | `github.com/visioncortex/visioncortex` (Apache-2.0), unas 124 estrellas, último push 2026-08. Es el núcleo de `vtracer` (`visioncortex/vtracer`, MIT, unas 7,3 k estrellas, activo: último push 2026-10-12). |
| `linesweeper` | 0.5.0 | MIT OR Apache-2.0 | Unión de contornos con relleno no-cero (`remove_overlap`). | `jneem/linesweeper`, unas 45 estrellas, último push 2026-01-25. Usa `kurbo` 0.13.1, la misma versión del workspace. |

- **`image`:** con `default-features = false` y solo las características `png` y `jpeg`, para reducir la superficie de ataque y de dependencias. FORMA impone sus propios límites antes de decodificar: ≤ 64 MiB de archivo y ≤ 50 megapíxeles (`LIMIT_EXCEEDED`). El lector de imágenes entra en el fuzzing nocturno como objetivo nuevo `image_decode`.
- **`visioncortex`:** FORMA hace su propia paleta, su máscara suave sobremuestreada ×4 y sus candidatos (spec §6.1); solo delega el paso de máscara binaria a curvas (modo `Spline`). Sus contornos pueden traer nodos de más, así que el reajuste de cúbicas (`fit`, implementación propia en `forma-geometry`) y los chequeos de geometría limpian el resultado.
- **`linesweeper`:** queda detrás del trait `PathOps` de `forma-geometry`. Si falla devuelve `INVALID_GEOMETRY`. Opera sobre trazados de `kurbo` y se espera que conserve las curvas [H: lo confirma el corpus de solapamientos con el que se valida al empezar la implementación, antes de apoyar nada más en ella]. Si el corpus falla, se pasa al plan B sin tocar a los llamadores.
- **Plan B: Skia PathOps** (robusta y muy usada en herramientas de fuentes), detrás del mismo trait `PathOps`. Se activa solo si `linesweeper` no supera el corpus; entonces se registra un ADR nuevo, porque supone una dependencia de C++.

Las tres son Rust y deben compilar en los tres sistemas operativos [H: lo confirma la CI de la matriz].

## Alternativas

- **potrace** (y sus enlaces): licencia GPL. **Prohibida** por `CLAUDE.md`; no se usa ni se enlaza.
- **Skia PathOps como primera opción** (lo previsto en `docs/03-architecture.md` para la fase 4): robusta, pero obliga a compilar C++ en los tres sistemas (builds distintos por SO) y a mantener un puente. Se conserva como **plan B** detrás del trait `PathOps`.
- **iOverlay** (Apache-2.0): licencia compatible, pero solo trabaja con **polígonos**. Habría que aplanar las curvas, unir y volver a ajustar cúbicas, con pérdida de fidelidad y nodos de más en cada glifo. Descartada.
- **Booleanas propias:** es el problema difícil que el registro de riesgos ya marca (T5), con casos degenerados que cuestan meses. Descartada.
- **Usar `vtracer` completo en lugar de `visioncortex`:** `vtracer` es una aplicación de imagen a SVG con su propio flujo de color y agrupamiento. FORMA necesita controlar la binarización (distancia en Lab a la tinta, sobremuestreo ×4, exclusión de figuras de otros colores), así que solo le sirve el vectorizador binario que `vtracer` usa por debajo (`visioncortex`).
- **Escribir el vectorizador:** el reajuste de cúbicas (Schneider) sí se implementa en casa porque lo usan también el trazo y la limpieza; la extracción de contornos y la detección de esquinas a partir de píxeles se delegan, porque ya están resueltas y probadas en `visioncortex` y `vtracer`.

## Consecuencias

- Tres dependencias directas nuevas, más sus transitivas. `cargo-deny` comprueba las licencias en la CI, con la lista permitida de `CLAUDE.md`.
- **`linesweeper` es joven** (0.5, repositorio de un solo usuario, `jneem`, unas 45 estrellas): riesgo T19. Mitigaciones: corpus de solapamientos al empezar, trait `PathOps` para cambiar de motor, pruebas de propiedades de `remove_overlap` (área ±0,5 % y sin autointersecciones) y versión fijada en `Cargo.lock`.
- **`visioncortex` tiene pocas estrellas** pero lo usa un proyecto muy usado y activo (`vtracer`), y su licencia permite mantener un fork si se abandonara.
- **Fidelidad limitada por la resolución:** con letras de unos 110 px el vectorizador no puede recuperar detalle que la imagen no tiene (T20). El sobremuestreo, el IoU por glifo y la corrección con `nodes_edit` lo acotan; no lo eliminan.
- El decodificador de imágenes es una superficie de ataque nueva: límites de tamaño, lectura solo dentro de las raíces permitidas y fuzzing nocturno (`image_decode`).
- `docs/03-architecture.md` (booleanas con Skia PathOps en la fase 4) queda desactualizado: ahora la primera opción es `linesweeper` y Skia PathOps es el plan B. Las booleanas generales (intersección, diferencia) quedan para el Hito 6.
