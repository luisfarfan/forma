# QA Hito 2 — Demostración con agentes reales (criterio 8)

> Fecha: _pendiente_ · Máquina: _pendiente_ · `forma` _pendiente_, instalado con `cargo install --path core/crates/forma-cli --locked` desde la rama `hito2`.
> Esta demostración es manual y no bloquea la CI. Los criterios automatizados del hito están en `docs/04-roadmap.md` y se verifican con `scripts/acceptance.sh` (en la CI de los tres SO).
> Las decisiones del controlador y lo diferido están en [`decisiones.md`](decisiones.md).
> **Solo se publica aquí la demo con la imagen de prueba propia** (generada por código, `tests/fixtures/hito2/specimen.png`). La imagen del alfabeto de la hermana del responsable y la fuente que resulta de ella se quedan en local, fuera del repositorio.

## Guion

_Pendiente (lo completa el controlador en la Tarea 21)._ Debe incluir:
- cómo se conectó cada agente a `forma mcp` (`--allow-dir`, `--journal-dir`);
- la instrucción exacta que recibió ("convierte este alfabeto en una fuente completa" y su variante en inglés);
- qué imagen se usó.

## Resultados

_Pendiente._ Tabla al estilo del Hito 1 (Claude Code y Codex): llamadas a herramientas FORMA y errores, flujo, `validate`, IoU por glifo, pares de kerning y colisiones, `charset_check` de `unicase-es`, TTF y WOFF2, OTS, fontTools, HarfBuzz, fontspector, carga en Chrome, duración y costo.

## Hallazgos

_Pendiente._

## Cómo repetirla

_Pendiente._ Ver `docs/guides/connect-agents.md` (sección "De una imagen a una fuente") y repetir los comandos de `docs/qa/hito1/README.md` con la instrucción del Hito 2.
