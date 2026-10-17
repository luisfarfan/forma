# ADR 0004: Formatos `.forma` (SQLite) y `.formapackage` (carpeta) con un esquema de entidades común

- Estado: aceptada (Hito 1)
- Fecha: 2026-10-16
- Referencias: `docs/02-tech-stack.md` §7, `docs/03-architecture.md` §8, `core/crates/forma-store`

## Contexto

Una fuente es el trabajo de meses: perderla o corromperla es lo peor que puede hacer un editor. Hacen falta un archivo único para trabajar y compartir, y un formato de texto que se pueda versionar con git y revisar en diffs. Ambos deben contener exactamente lo mismo y convertirse sin pérdidas.

## Decisión

- **Esquema común de entidades:** la fuente sin glifos (`font`) y un documento por glifo, en JSON canónico (claves ordenadas de forma explícita y recursiva, sangría de 2 espacios, `\n` final; los `f64` se leen con `float_roundtrip`, así que un número guardado vuelve idéntico). Guardar dos veces la misma fuente da los mismos bytes. Los datos que el modelo no interpreta se conservan en `lib`.
- **`.forma`:** un único archivo SQLite con `application_id = 0x464F524D` ("FORM"), `user_version = 1` y `journal_mode = DELETE` (un solo archivo, sin `-wal`). Tablas `manifest` (clave/valor: `format`, `format_version`, `font_id`, `generator`) y `entities` (`kind`, `id`, `json`). Se construye desde cero en un archivo temporal, en una transacción, y se renombra sobre el destino.
- **`.formapackage`:** carpeta con `manifest.json`, `font.json` y `glyphs/*.json`. El nombre de cada archivo de glifo se deriva del nombre del glifo (mayúsculas como `A_`, caracteres ilegales como `_`, nombres de dispositivo de Windows protegidos, colisiones sin distinguir mayúsculas con `#2`, `#3`…), pero **al cargar se ignora**: manda el `id` del documento.
- `pack` y `unpack` (`forma_store::{pack, unpack}`) convierten en cualquier dirección según la extensión; la fuente de origen se carga y valida entera antes de escribir nada.
- **Durabilidad.** Nunca se edita en el sitio. Cada archivo se escribe a un temporal vecino, se vacía a disco (`fsync`) y se renombra (atómico), y luego se vacía el directorio. Un paquete usa "preparar y confirmar": primero todos los temporales, después los renombres (`manifest.json`, glifos y `font.json` al final) con reversión si uno falla, y por último se borran los archivos de glifos obsoletos. Un error antes de confirmar deja el paquete como estaba; solo un corte de luz dentro de la ráfaga de renombres (milisegundos) puede dejar archivos nuevos y viejos mezclados, y está documentado en `package.rs`.
- **Rechazos explícitos.** `load` ejecuta siempre `validate_invariants`; una versión de formato mayor que 1 es `UnsupportedVersion`; guardar un paquete en una carpeta que no está vacía ni es un paquete es un error y no toca nada; **se rechazan los enlaces simbólicos dentro de un paquete** (manifiesto, `font.json`, `glyphs/` y sus entradas; `StoreError::SymlinkInPackage`, que el motor informa como `PATH_NOT_ALLOWED`), porque uno podría hacer que una carga lea o un guardado escriba o borre fuera del paquete.
- **Diario de recuperación** (`journal`): tras cada cambio confirmado, el motor agrega una línea JSON con el *estado* de las entidades tocadas (aplicarla dos veces da lo mismo) a `<carpeta>/<id de la fuente>-<hash>.journal`, con `sync_data`. El diario pertenece a un **archivo**, no solo a una fuente: `<hash>` son los 16 primeros dígitos hexadecimales del SHA-256 de la ruta canónica del documento, y cada registro repite esa ruta (`document`), de modo que dos copias de una fuente nunca comparten diario y un diario copiado o renombrado no se ofrece para otro archivo. Se vacía al guardar (guardar en otra ruta lo traslada) y se borra al cerrar. Al abrir un documento con registros pendientes, `document_open` devuelve `recovery_available` y acepta `recover: true`. Tolera una última línea truncada por un corte; cualquier otra línea ilegible es corrupción.
- **Escritor único** (`03-architecture.md §7`): el motor que abre o crea un documento con archivo toma un bloqueo exclusivo del SO (`File::try_lock`) sobre `<carpeta de bloqueos>/<sha256 de la ruta>.lock` y lo mantiene hasta cerrarlo (o hasta que el proceso muere). Otro proceso con la misma carpeta abre el documento **solo lectura** (`read_only`, `locked_by_other_process`): lee, previsualiza, valida y exporta a archivos nuevos, pero todo cambio, guardado, exportación que sobrescribe o cierre que descarta devuelve `UNSUPPORTED`. Solo quien tiene el bloqueo abre, amplía, vacía o borra el diario del archivo; un servidor `--read-only` nunca bloquea ni toca diarios. La CLI y `forma mcp` usan por defecto `<datos de la app>/locks` (`--lock-dir` o `FORMA_LOCK_DIR` para cambiarla): dos procesos solo se excluyen si comparten carpeta.
- **Pruebas.** Round-trip de ambos formatos y `pack ↔ unpack` con proptest (500 casos), pruebas de fallo en la escritura atómica con escritor inyectable, y fuzzing de `forma_store::load` con los objetivos `load_package` y `load_forma` (`core/fuzz`, ejecutados cada noche). La CI corre además las pruebas de `forma-store` con `FORMA_REAL_FSYNC=1`: sus pruebas normales usan la característica `test-no-fsync` para no pasar minutos en `fsync`.

## Alternativas

- **Zip de JSON (como Sketch):** un archivo, pero no se puede versionar ni revisar con git, y la actualización parcial reescribe todo.
- **Solo carpeta:** ideal para git, pero incómoda de mover o adjuntar, y el guardado de muchos archivos no es atómico.
- **Solo SQLite, con WAL:** más rápido para guardados pequeños, pero deja archivos auxiliares y rompe la idea de "un archivo".
- **JSON de un solo archivo:** diffs enormes y un guardado que reescribe toda la fuente.

## Consecuencias

- El formato es parte del contrato: cambiarlo exige subir `format_version`/`user_version` y escribir una migración pura y probada.
- Un `.forma` se reescribe entero en cada guardado (aceptable con miles de glifos; un perfil de rendimiento llegará en el Hito 2).
- La caja de rutas (`--allow-dir`) y el rechazo de enlaces simbólicos se complementan: la primera acota dónde se puede abrir, el segundo qué hay dentro de un paquete.
- En FAT/exFAT o redes sin `rename` atómico las garantías son las del sistema de archivos; no se prometen más.
