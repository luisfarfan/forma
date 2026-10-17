# Decisiones tomadas durante la ejecución del Hito 1

> Registro de las decisiones ("rulings") que tomó el controlador al ejecutar el plan `docs/superpowers/plans/2026-10-16-hito1-headless-core-mcp.md` con subagentes. Cada una indica qué se decidió y, cuando aplica, qué cuesta si resulta equivocada. Las decisiones arquitectónicas mayores están además en `docs/adr/`.

## Proceso
1. **Rama `hito1` en el mismo checkout, sin worktree.** Es el único esfuerzo en curso; `main` queda intacta. Costo si es incorrecta: ninguno.
2. **El plan manda frente al roadmap en el número de herramientas:** son 35, no "~28".
3. **Atribución de commits de subagentes:** los subagentes pueden firmar con su propio modelo.
4. **La demo con agentes reales (Task 22) la ejecuta el controlador.**

## Modelo y geometría
5. **Nombres de glifo válidos:** `.notdef`, o bien 1–63 caracteres ASCII 0x21–0x7E que no empiecen por dígito ni por `.`. Costo: rechaza nombres como `.null` (fácil de relajar).
6. **Ninguna creación ni edición normaliza la dirección de los contornos.** Solo lo hacen `geometry_fix` y `reverse`. Sin esto, el criterio "detectar el contorno invertido a propósito" sería imposible.
7. **Coordenadas no finitas:** se rechazan en los comandos. Si el problema está en el argumento, `INVALID_ARGUMENT`; si la geometría resultante es inválida, `INVALID_GEOMETRY`.
8. **Nuevo tipo de problema `InvalidContour`:** marca los contornos estructuralmente inválidos en lugar de saltárselos.
9. **`DuplicatePoints`:** se definen como nodos on-curve consecutivos idénticos. El orden de las correcciones es duplicados → extremos → grilla → dirección.
10. **`fix_paths` itera hasta que no quede nada por corregir (máximo 6 pasadas).** Con una sola pasada, la idempotencia fallaba en el 8 % de los casos.
11. **Semántica de comandos:**
    - Los PathIds se renumeran.
    - El master por defecto es el primero, y `CreateGlyph` crea una capa en cada master.
    - LSB desplaza el glifo y conserva el RSB; RSB fija el advance; `units_per_em` no escala.
    - `Move` mueve solo los índices indicados; `Delete` fusiona los segmentos adyacentes; `Insert` define el índice de segmento con wrap.
12. **`GlyphRef`:** primero se busca por id y, si no existe, por nombre.
13. **Contornos casi sin área:** nunca se marcan como de dirección incorrecta, para que los agentes no entren en un bucle.

## Historial y concurrencia
14. **`GlyphOrder` nunca genera conflicto.** Crear o borrar glifos distintos en paralelo no choca.
15. **`revert` reconcilia el orden de glifos en lugar de copiarlo.**
16. **Historial lineal global:**
    - `undo`/`redo` solo sobre la última transacción del mismo actor.
    - `revert` acepta cualquier tipo de entrada.
    - `restore` exige `base_rev` y comprueba conflictos por entidad.
17. **`history_undo`/`redo`/`plan_commit`/`plan_discard` están exentos de `base_rev`.**
18. **Un `NOT_FOUND` causado por un glifo borrado después de `base_rev` se devuelve como `REVISION_CONFLICT`.** Los detalles del conflicto incluyen `name`.

## Persistencia y recuperación
19. **JSON canónico con claves ordenadas explícitamente.** No se depende de la configuración de `serde_json`.
20. **Al cargar siempre se validan las invariantes del documento.**
21. **Nunca se sobrescriben carpetas ajenas.** Los guardados de paquete son duraderos (fsync) y todo-o-nada, y un primer guardado fallido se puede reintentar.
22. **Los paquetes rechazan symlinks internos** (`PATH_NOT_ALLOWED`) e ignoran archivos ocultos `._*` y `.DS_Store`.
23. **Journal:**
    - Una línea JSON por registro; solo se tolera una última línea incompleta.
    - Se conecta a todas las mutaciones; se vacía al guardar.
    - Tras cualquier fallo de escritura pasa a `Stale` y lo indica con `journal_warning`.
    - El handle se libera antes de borrar el archivo (Windows).
24. **Escritor único entre procesos:**
    - Bloqueo por documento con `File::try_lock` en una carpeta de locks.
    - El segundo proceso que abre el documento lo recibe en solo lectura.
    - Los journals se identifican por fuente y ruta, y solo los toca quien tiene el lock.
    - Costo: si dos procesos usan carpetas de lock distintas, no se excluyen entre sí.
25. **Guardar sobre un archivo que cambió en disco es destructivo y requiere plan.**
26. **Errores de E/S de SQLite e interop se devuelven como `IO_ERROR`.**

## Compilación, validación y render
27. **`read-fonts` duplicado aceptado inicialmente.** Desapareció al desactivar la feature `text` de vello_cpu.
28. **WOFF2 en Rust puro (`ttf2woff2`) en lugar de woofwoof.** woofwoof compilaba C++ con flags de GCC y habría roto Windows.
29. **`.notdef` automático:**
    - Rectángulo con contraluz y advance UPM/2.
    - Siempre en la posición 0.
    - `space` se añade solo si falta.
    - El preflight rechaza valores fuera de los rangos OpenType.
30. **Validación en el producto con chequeos propios.** OTS, fontspector y HarfBuzz se usan como oráculos en CI (ADR 0006).
31. **Previews:**
    - Encuadre por métricas, relleno NonZero, render en un solo hilo.
    - Un encuadre degenerado da error.
    - `glyph_svg` devuelve `Result`.

## Motor, aprobaciones y MCP
32. **El registro declara las 35 herramientas desde la Task 13**, con placeholders que se reemplazaron en las tareas 14–16.
33. **Argumentos con `deny_unknown_fields`.** Las rutas relativas se resuelven contra la primera raíz, y `document_open` es idempotente.
34. **`document_close` está permitido en modo solo lectura.** En ese modo se ocultan las herramientas que se rechazarían.
35. **El filtrado de toolsets lo hace el motor, no la fachada.** `core` siempre está activo en `forma mcp`.
36. **Los límites de 4 MiB por texto y de anidamiento se aplican en el motor a todas las herramientas.**
37. **Efectos destructivos:**
    - Se calculan paso a paso sobre el estado borrador y cuentan glifos distintos.
    - Reemplazar el contorno de un glifo creado en la misma llamada no es destructivo.
    - Sí lo son: quitar el último contorno de una capa dibujada, aplicar una transformación singular, un `revert` que borra, y un cierre con `discard`.
38. **Planes:**
    - Token HMAC ligado a actor, operación, `base_rev` y TTL; de un solo uso.
    - Los planes de escritura de archivos fijan la revisión.
    - Hay un chequeo de conflicto antes de emitir un plan.
    - Los diffs se acotan a 100 entradas por lista.
    - El almacén tiene tope de 1000 planes y 64 MiB.
39. **`redo` de un cambio ya aprobado no requiere un nuevo plan.**
40. **Respuestas acotadas:**
    - Listas `touched` de hasta 100 entradas.
    - `geometry_check`, `validate` y `metrics_get` limitados a 200 entradas y 32 KiB, con totales exactos.
    - `preview_text` admite un máximo de 4096 caracteres.
41. **Anotación `destructiveHint`** en todas las herramientas que pueden requerir aprobación. Hallazgo de la demo: Codex pide aprobación por ellas, así que hay que revisar esto en el Hito 2.
42. **MCP:**
    - Se aceptan todas las versiones del protocolo que soporta rmcp.
    - `-32602` para recursos desconocidos.
    - Las imágenes van sin `structuredContent`.
43. **CLI:**
    - Raíces de la sandbox = directorio actual + rutas explícitas.
    - `--yes` confirma el plan devuelto por esa misma invocación.
    - `pack`/`unpack` no sobrescriben sin `--yes`.
    - `--allow-dir` inexistente aborta.
44. **El criterio 4/5 (tokens caducados o ajenos, nombres con "instrucciones") se cumple con tests del motor.** La suite stdio cubre el camino de punta a punta.
45. **Paridad con fontmake definida geométricamente:** glifos, cmap y avances exactos; bbox ±1; área ±0,5 %.
46. **Cobertura medida (≥ 95 %).** Un test de handle retenido no es observable en Unix, así que lo cubre el job de Windows en CI.

## Tras publicar el repositorio (primera CI en Windows)
47. **La primera ejecución en Windows falló con "Access is denied (os error 5)" al compilar.** Windows no deja renombrar una carpeta mientras un archivo dentro está abierto, y los antivirus abren los archivos recién escritos.
    - La compilación escribe su UFO directamente en su carpeta temporal privada (`write_ufo_unstaged`), sin renombrar nada.
    - La exportación UFO para el usuario conserva el staging todo-o-nada, pero reintenta el renombrado en Windows durante unos 2 s.
    - La CI usa `cargo test --no-fail-fast`: el primer fallo había ocultado el resto del workspace en Windows.
    - Costo si es incorrecta: un renombrado legítimamente prohibido tarda unos 2 s más en fallar.
48. **Los tests de symlinks se ignoran en Windows** (crear symlinks exige privilegios). Ahí los cubren los tests de junctions, que pasan.

## Diferido al Hito 2 (sin pérdida de datos)
- Save-as libera el lock antiguo antes de borrar su journal.
- Un `stat` fallido tras guardar cuenta como "cambiado".
- `pack`/`unpack` no toman el lock.
- La CLI no lleva journal.
- El motor mantiene el lock global durante la compilación.
- Faltan API tipada, agrupación de gestos y retención de snapshots.
- Portabilidad de schemas para clientes de tipo Gemini.
- Tamaño de `tools/list`.
