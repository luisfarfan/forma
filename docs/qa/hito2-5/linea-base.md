# Hito 2.5 — Línea base (modo A, pipeline del Hito 2)

Medida con `tools/bench/agent_metrics.py` sobre las **transcripciones existentes**. No se repite el flujo antiguo (decisión del responsable, §11.5 de la especificación).

## Claude Code (Opus 5.5)

| Métrica | Alphabetical (alfabeto privado, 26 letras + 52 glifos diseñados) | Specimen (8 letras propias + composición) |
|---|---|---|
| Turnos | 173 | 43 |
| Duración total | 986,4 s | 153,1 s |
| Llamadas a herramientas | 172 | 42 |
| Bytes de argumentos | 79 779 | 6 254 |
| Bytes de resultados | 111 875 | 14 775 |
| Imágenes recibidas | 95 | 2 |
| Tokens de entrada (sin caché) | 166 | 52 |
| Caché leída | 14 055 749 | 1 958 776 |
| Caché escrita | 262 418 | 91 750 |
| Salida | 101 318 | 10 898 |
| Razonamiento (incluido en salida) | 35 708 | 2 464 |
| Costo (reportado por el cliente) | **6,94 USD** | **1,34 USD** |

**Herramientas más usadas en Alphabetical:**

| Herramienta | Llamadas |
|---|---|
| `preview_glyph` | 77 |
| `edit_batch` | 13 |
| `preview_text` | 9 |
| `spacing_auto` | 9 |
| `trace_compare` | 8 |
| `glyph_measure` | 6 |
| `ToolSearch` | 4 |

**Notas:**
- **Tiempo del motor:** las transcripciones no traen el tiempo del motor. Medido por CLI sobre la misma imagen, es de ~1,2 s (auditoría del 2026-10-18); el resto es tiempo del agente.
- **Privacidad:** la transcripción de Alphabetical está en `~/fonts/hermana/` y **no** se copia al repositorio. Solo se publican estas cifras agregadas.

## Codex

No existe una transcripción cruda de Codex con el flujo imagen → fuente: la del Hito 1 solo se guardó condensada. Por eso se hace **una corrida A sobre S**, con tope de turnos, en el bloque F, junto con las corridas del modo B (presupuesto total ≤ US$ 15).

## Tamaño del catálogo (`forma mcp --print-catalog-size`, estimación ÷4)

| Configuración | Herramientas | Caracteres | De ellos, output schemas | Tokens estimados |
|---|---|---|---|---|
| Hito 2 (`main` @ `26a60f3`) | 50 | 314 636 | 188 036 | ~79 k |
| Hito 2.5, por defecto (bloque A) | 51 | 337 363 | 203 120 | ~84 k |
| Hito 2.5, `--profile image-font --output-schemas none` | 23 | 48 367 | 0 | ~12 k |

Lo que cuesta el catálogo en cada turno depende del cliente: Claude Code carga las herramientas de forma diferida (ToolSearch) y Codex las carga enteras. **Se mide por cliente en el benchmark (bloque F); estas cifras no prometen ningún ahorro.**
