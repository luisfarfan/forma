# Hito 2.5: calibración de umbrales (Task 18b)

Herramienta: `forma-bench calibrate --levels s,m,m-hand,l,o [--part fidelity|style|both] --out <dir>`. Escribe `calibration.json`. Todo es determinista y sin agente: el corpus se genera con código y con fuentes OFL. **El alfabeto privado no participa.**

Criterio (§8.5 de la especificación): entre los valores que no dejan pasar defectos, se elige el que más conserva la irregularidad del nivel O. Un valor que relaje la fidelidad respecto a la especificación se consulta antes al responsable.

## 1. Fidelidad geométrica (puertas de calidad del trazado, §4)

**Método:**
- Cada glifo del corpus se traza como lo traza el motor.
- **Glifo bueno:** el que coincide con su verdad, con IoU ≥ 0,97 contra la forma verdadera y los mismos contrahuecos.
- **Defectos inyectados** sobre los contornos buenos:

  | Defecto | Qué hace |
  |---|---|
  | `close_counter` | cierra el mayor contrahueco |
  | `drop_piece` | quita la pieza menor |
  | `spike_3px` | lleva un nodo 3 px afuera |
  | `shift_1px` | desplaza el contorno 1 px |
  | `bold_30` | +30 % de grosor, uniendo una banda de 0,3 trazos |

- **Juicio:** los defectos se juzgan con `forma_trace::assess_batch`, las mismas puertas del trazador.
- **Barrido:** para cada puerta, contra cada umbral, la proporción de glifos buenos marcados (falsos positivos), la de defectos que atraviesan todas las puertas (falsos negativos) y la irregularidad conservada en O.
- **Descartado:** la simplificación (`simplify_path` a 2 y 6 px) como defecto. Las propias salvaguardas de `simplify_path` rechazan los cambios grandes, de modo que el resultado queda dentro de 1,3 px y no es un defecto visible.

**Resultados (todo el corpus: S, M, M-hand, L y O; binario con la corrección C1 de la revisión del bloque E):**

| | Valor |
|---|---|
| Glifos / buenos / buenos orgánicos (O) | 1327 / 1093 / 28 |
| Defectos aplicados | 1093 `bold_30`, 287 `close_counter`, 9 `drop_piece`, 1093 `shift_1px`, 1093 `spike_3px` (3575 en total) |
| Defectos no detectados con los umbrales actuales | **0** |
| Glifos buenos marcados con los umbrales actuales | 1,1 % (12) |
| Irregularidad de O conservada | 100 % |

Los 12 buenos marcados son, casi todos, glifos de Fraunces a 60 px con IoU entre 0,953 y 0,968: a ese tamaño, sus remates finos ocupan pocos píxeles. Se suma un cruce de contorno en un caso rotado de EB Garamond. Todos quedan para revisión, ninguno se rechaza.

**Distribución en los glifos buenos** (1093):

| Métrica | p50 | p90 | p99 | máx. |
|---|---|---|---|---|
| 1 − IoU | 0,011 | 0,018 | 0,027 | 0,048 |
| Hausdorff p95 (px) | 0,20 | 0,25 | 0,31 | 0,38 |
| Hausdorff máx. (px) | 0,52 | 1,03 | 1,69 | 2,03 |
| Desviación de ajuste p99 (px) | 0,28 | 0,36 | 0,49 | 0,66 |

**Barridos**, cada umbral con las demás puertas en su valor actual:

| Puerta | Valor | Buenos marcados | Defectos no detectados | Irregularidad de O |
|---|---|---|---|---|
| IoU mínimo | 0,95 / 0,96 / **0,97** / 0,98 / 0,985 | 0,4 % / 0,7 % / **1,1 %** / 5,5 % / 22,9 % | 0 en todos | 100 % (96,4 % con 0,985) |
| Hausdorff p95 | 0,5 / 0,6 / **0,75** / 1,0 / 1,25 px | 1,1 % | 0 / 0 / **0** / 1 `shift_1px` / 1 | 100 % |
| Hausdorff máximo | 1,5 / **2,0** / 2,5 / 3,0 px | 2,2 % / **1,1 %** / 1,1 % / 1,1 % | 0 / **0** / 0 / 9 `spike_3px` | 100 % |
| Desviación de ajuste (fiel) | 0,35 / **0,5** / 0,75 / 1,0 px | 4,7 % / **1,1 %** / 1,0 % / 1,0 % | 0 en todos | 89,3 % / **100 %** / 100 % / 100 % |

**Decisión: los valores actuales (`QualityThresholds` versión 1) no cambian.** Cumplen el criterio de §8.5: ningún defecto topológico ni de rasgo fino pasa sin detectarse.
- **IoU, p95 y máximo:** cada uno está en el punto más estricto que no marca más glifos buenos sin ganar detección. Un valor más laxo deja pasar defectos (p95 de 1 px, máximo de 3 px).
- **Desviación de ajuste:** podría subirse a 0,75–1,0 px sin perder detección en estos defectos. Se mantiene en 0,5 px porque relajarla compromete la fidelidad y requiere la decisión del responsable (decisión 48).

## 2. Calidad tipográfica de la síntesis (puntuación de estilo, §5.5)

**Método:**
- Con los glifos trazados de cada alfabeto se construye el kit de estilo; son 56 fuentes de S, M, M-hand y O.
- **Bueno:** cada receta se dibuja con el kit verdadero.
- **Variantes con un kit erróneo en un aspecto**, puntuadas siempre contra el kit verdadero con el juicio completo (componentes, estructura y autoprueba):
  - trazo ×0,7 o ×1,3;
  - finales de la otra clase (planos ↔ redondos);
  - bolas espurias en una fuente de finales lisos.

**Resultados** (1960 glifos buenos):

| Variante | Aceptados (umbral 0,75, piso 0,5 y estructura) | Total ≥ 0,7 | ≥ 0,75 | ≥ 0,8 | ≥ 0,85 |
|---|---|---|---|---|---|
| Bueno | **85,1 %** | 92,8 % | 92,5 % | 89,8 % | 85,2 % |
| Finales de la otra clase | 24,4 % | 58,6 % | 40,3 % | 27,6 % | 24,0 % |
| Bolas espurias | 8,8 % | 55,9 % | 37,9 % | 17,2 % | 9,1 % |
| Trazo ×0,7 | 21,9 % | 26,7 % | 22,9 % | 18,3 % | 17,3 % |
| Trazo ×1,3 | 23,0 % | 28,8 % | 26,4 % | 23,1 % | 20,9 % |

**Lectura:**
- El piso por componente y el control de estructura rechazan mucho más que el total solo. Con bolas espurias, por ejemplo, el total ≥ 0,75 deja pasar el 37,9 %, y el juicio completo solo el 8,8 %.
- **Variantes aceptadas indetectables, no fallos del juicio:** el 0 y el 8 no tienen finales (la clase de final no cambia nada), ni los puntos ni la coma tienen trazo que comparar.
- **Fuentes de contraste o con remates (EB Garamond, Fraunces):** el peso de sus letras varía tanto que ±30 % cae dentro de su dispersión. En esas fuentes la autoprueba (≈ 0,27) envía todo glifo sintetizado a revisión. Ninguna variante errónea pasa sin una marca.
- **Glifos buenos rechazados (15 %):** sobre todo `* 6 9` (finales junto a un cruce), la tilde (sale de su caja en algunas fuentes) y algunos anchos en fuentes con mucho contraste. Todos piden diseño, como manda el principio 5: mejor pedir ayuda que producir un glifo mediocre.

**Umbral:** se mantiene **0,75** (piso 0,5), experimental. Subir a 0,8 rechaza el 4,7 % más de glifos buenos y reduce poco las variantes aceptadas con el juicio completo.

### Autoprueba de reproducción (decisión 57)

Mediana del solapamiento entre las letras I, H, O, L, E y T de la fuente y las mismas letras dibujadas con esqueletos en su estilo:

| Fuentes | Solapamiento |
|---|---|
| Inter | 0,92–0,97 |
| Patrick Hand | 0,85–0,88 |
| specimen | 0,84 |
| casos orgánicos (bulb, hand_pressure, rough_brush, inline, complex) | 0,50–0,75 |
| imágenes rotadas 15° | ≈ 0,62 |
| EB Garamond y Fraunces | 0,24–0,43 |

**Umbral: 0,9** (modo conservador). Solo una fuente que los esqueletos reproducen casi exactamente recibe glifos sintetizados sin una petición de mirada. Las manuscritas (Patrick Hand) y las orgánicas no se homogeneizan en silencio.
