# Fixtures del Hito 2

Material propio, bajo `MIT OR Apache-2.0` (como el resto del repositorio).

- `truth.json`: contornos verdaderos de ocho letras (`A B C H O F M R`), UPM 1000, altura de mayuscula 700, en el formato de glifo del Hito 1 (`name`, `unicodes`, `advance_width`, `svg_path`).
  - `A B C H O` se copian de `tests/fixtures/hito1/glyphs.json` (dibujos propios).
  - `F M R` se extraen de `docs/assets/forma-wordmark.forma` (hecho con la CLI de FORMA) con `forma call glyph_get`.
- `specimen.png`: imagen de 1600 x 700 px generada por codigo a partir de `truth.json` (fondo `#111111`, tinta `#FF5A1F`, circulo azul `#2A5BFF`). No contiene material de terceros. La genera `core/crates/forma-trace/tests/common/mod.rs`; se regenera con `FORMA_REGENERATE_FIXTURES=1 cargo test -p forma-trace --test specimen`.
