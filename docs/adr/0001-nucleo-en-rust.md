# ADR 0001: Núcleo en Rust; la lógica tipográfica vive solo en el núcleo

- Estado: aceptada (Hito 1)
- Fecha: 2026-10-16
- Referencias: `docs/02-tech-stack.md` §1 y §3, `docs/03-architecture.md` §1 y §3

## Contexto

FORMA debe funcionar igual en macOS, Windows y Linux, y todo lo que haga la persona desde la interfaz debe poder hacerlo también un agente (MCP) o un script (CLI), con las mismas garantías. El ecosistema de compilación y lectura de fuentes que se quiere reutilizar (fontc, fontations, norad, kurbo) está escrito en Rust y es de licencia permisiva. El toolkit de UI aún no está decidido (Hito 2), así que el núcleo no puede depender de él.

## Decisión

- El núcleo es un workspace de Rust en `core/` (`forma-*`), con `edition = "2024"`, licencia `MIT OR Apache-2.0` en cada crate y la toolchain fijada en `rust-toolchain.toml` (1.99.1, con rustfmt y clippy).
- **Toda la lógica tipográfica vive en el núcleo.** `forma-cli` y `forma-mcp` son fachadas finas: traducen transporte a llamadas del motor y no contienen reglas del dominio. Lo mismo valdrá para la interfaz y el agente integrado.
- El núcleo no depende de ningún sistema operativo. La CI compila y prueba el workspace completo en macOS, Windows y Ubuntu (más una comprobación de compilación para macOS Intel).
- Los crates son `#![forbid(unsafe_code)]`, sin `unwrap()`/`expect()` en código de librería y con errores `thiserror`. Las dependencias nuevas se justifican por licencia y mantenimiento, y `cargo deny check licenses` las controla en cada CI.

## Alternativas

- **Núcleo en C++ o Swift con la UI nativa de cada sistema:** habría tres bases de código para una misma lógica, y Swift limitaría el producto a Apple.
- **Núcleo en Python o TypeScript:** buena velocidad de desarrollo, pero sin reutilizar fontc/fontations y con más riesgo para el rendimiento del canvas.
- **Reutilizar fontmake/fontTools como motor:** dependencia de Python en el producto; se conservan solo como oráculos de prueba (ADR 0006).

## Consecuencias

- Una sola implementación de cada operación, probada una vez; la UI futura y el agente integrado la heredan.
- El shell de UI y `forma-platform` tendrán que cargar con todo lo específico de cada plataforma.
- La toolchain fijada hace reproducible la CI, pero obliga a actualizarla a propósito.
- Los crates que dependen de código C (SQLite embebido, vía rusqlite) necesitan compilador de C en los tres sistemas; la CI lo comprueba.
