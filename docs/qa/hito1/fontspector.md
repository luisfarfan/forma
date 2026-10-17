# fontspector sobre el TTF del escenario (Hito 1, criterio 2)

> Ejecución local: macOS (arm64), 2026-10-16, `scripts/acceptance.sh`, paso 5.
> fontspector 1.9.0 (`cargo install fontspector --locked`). Comando: `fontspector --profile opentype --error-code-on fail --json out/acceptance/fontspector.json out/acceptance/forma-demo.ttf`.

## Resultado: 0 FAIL

Se ejecutaron 49 comprobaciones:

| Estado | Cantidad |
|---|---:|
| PASS | 29 |
| WARN | 2 |
| SKIP | 18 |
| INFO | 0 |
| FAIL | **0** |
| FATAL | 0 |
| ERROR | 0 |

La aceptación exige 0 FAIL. Con `--error-code-on fail`, fontspector sale con 1 si alguna comprobación termina en FAIL, FATAL o ERROR, y el script se detiene. Lo verifiqué con una copia del TTF que tenía `head.macStyle` en negrita: dio 3 FAIL y salió con 1.

## WARN (justificados)

| Comprobación | Mensaje | Justificación |
|---|---|---|
| `opentype/STAT/ital_axis` | `no-stat-table`: Static font is missing the 'STAT' table. | La tabla STAT solo es obligatoria en las fuentes variables, y esta es una fuente estática de un solo estilo. fontmake tampoco genera STAT con el mismo UFO. Pasará a ser obligatoria con las variables (fase 3). |
| `opentype/fsselection_wws` | `no-wws-without-wws-names`: OS/2 fsSelection WWS bit is not set, and the font does not have name IDs 21/22. | Es una recomendación, no un requisito. El bit WWS afirma que los nombres de la familia siguen el modelo peso/anchura/pendiente, y FORMA aún no modela familias de estilos (tiene un solo master, «Regular»). fontmake produce el mismo `OS/2` con el mismo UFO (ver `fontmake-parity.md`). Se revisará cuando FORMA gestione familias. |

## SKIP (no aplican a esta fuente)

- **11 comprobaciones de fuentes variables:** `fvar/*`, `varfont/*`, `slant_direction`, `weight_class_fvar`.
- **3 de `GDEF`:** la fuente no tiene GDEF porque no tiene marcas ni features.
- **`name/postscript_vs_cff`:** solo aplica a CFF.
- **`family/underline_thickness`:** necesita fuentes hermanas en la familia.
- **`varfont/family_axis_ranges`:** necesita varias fuentes variables.
- **`vendor_id`:** necesita un `vendor_id` en `fontspector.toml`, que el proyecto aún no fija.
