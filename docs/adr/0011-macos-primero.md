# ADR 0011: macOS primero; Windows y Linux se retoman antes de la v1.0

- Estado: aceptada (Hito 2.5)
- Fecha: 2026-11-02
- Referencias: `CLAUDE.md` (regla de arquitectura 6), `docs/04-roadmap.md` (definición de "terminado", Hito 9), `.github/workflows/ci.yml`, `.github/workflows/nightly.yml`, ADR 0001, ADR 0008

## Contexto

Desde el Hito 1, una funcionalidad solo cuenta como terminada si pasa en la CI de macOS, Windows y Ubuntu 24.04 (regla 6 y punto 5 de la definición de "terminado"). El responsable y los agentes trabajan en macOS. Windows y Linux solo se ejercitan en la CI.

Hechos observados (2026-10-19 a 2026-11-02):
- El Hito 2.5 llegó a `main` (`d400aaa`) con macOS y Windows en verde. En Ubuntu, el job se canceló al llegar al límite de 90 minutos con el test `font_from_image` todavía en marcha. En Ubuntu el job además ejecuta la suite completa instrumentada para cobertura (`cargo llvm-cov`), fmt, clippy, licencias, fsync real, fontspector y MCP Inspector.
- Los últimos arreglos antes del cierre (`3c55705`: copias de seguridad y rutas en Windows) fueron problemas que solo aparecían en la CI, en sistemas que nadie usa a diario.
- Cada push tarda entre 30 y 90 minutos en dar un resultado, y el hito queda bloqueado por sistemas que todavía no tienen usuarios.

## Decisión

1. **macOS es la plataforma que decide "terminado"** hasta el Hito 9. El punto 5 de la definición de "terminado" pasa a ser "pasa en la CI de macOS".
2. **FORMA sigue siendo multiplataforma como objetivo.** Windows y Linux se retoman antes de la v1.0: el Hito 9 (distribución) incluye volver a poner los tres sistemas en la CI y corregir lo que haya divergido.
3. **El núcleo sigue sin dependencias de sistema operativo** (regla 6, primera mitad). Sale casi gratis y mantiene barato el regreso a Windows y Linux. Lo específico de una plataforma sigue yendo en `forma-platform` o en el shell. Lo que deja de exigirse es que compile y pase en Windows y Ubuntu en cada cambio.
4. **CI:**
   - `ci.yml` corre solo en `macos-15`. Las verificaciones que hacía solo Ubuntu se trasladan a macOS: fmt, clippy, licencias (`cargo-deny`), fsync real, fontspector, MCP Inspector y la puerta de cobertura.
   - Se mantiene la comprobación de compilación para Intel (`x86_64-apple-darwin`).
   - El script `scripts/acceptance.ps1` se conserva sin ejecutar, para retomarlo en el Hito 9.
   - El fuzzing nocturno sigue en Ubuntu: prueba lógica independiente del sistema y no es una verificación de soporte de Linux.
   - La regresión nocturna de los niveles M y M-hand pasa a macOS: es una puerta de calidad del producto.

## Alternativas

- **Solo macOS, para siempre:** cambiaría el objetivo del producto (alternativa multiplataforma a Glyphs, que es solo para Mac) y la elección de Qt para la UI. Descartada por el responsable.
- **Mantener Windows y Ubuntu sin bloquear** (`continue-on-error`): avisarían pronto de divergencias, pero con fallos que nadie atiende y más minutos de CI. Descartada por el responsable.
- **Mantener los tres sistemas y solo acelerar Ubuntu** (cobertura aparte o límite más alto): conserva la garantía, pero sigue bloqueando cada hito con sistemas que nadie usa todavía. Descartada.

## Consecuencias

- Los hitos se cierran con la CI de macOS en verde. Baja el tiempo de cada push y se elimina una fuente de bloqueos.
- **Riesgo nuevo:** se pueden acumular divergencias en Windows y Linux (rutas, fin de línea, fsync, fuentes del sistema, Chrome). Se mitiga con la regla 6 (núcleo sin dependencias de SO) y con el trabajo de retomarlos en el Hito 9. Se registra en `docs/06-risks.md`.
- El criterio 6 del Hito 2 (WOFF2 en Chrome sin interfaz) solo se verificaba en Ubuntu. En macOS se activa si el runner lo permite. Si no, queda verificado en local y pendiente para el Hito 9.
- Documentos actualizados: `CLAUDE.md`, `docs/04-roadmap.md`, `docs/03-architecture.md` (pruebas) y `docs/06-risks.md`.
