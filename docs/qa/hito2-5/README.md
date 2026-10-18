# Hito 2.5: imagen a fuente autónoma (estado y evidencia)

Rama `hito2-5`. Especificación: `docs/superpowers/specs/2026-10-18-hito2-5-imagen-a-fuente-autonoma-design.md` (enmiendas en §12). Plan: `docs/superpowers/plans/2026-10-18-hito2-5-imagen-a-fuente-autonoma.md`. ADR 0010.

| Documento | Contenido |
|---|---|
| `linea-base.md` | la línea base medida (transcripciones del Hito 2) |
| `decisiones.md` | decisiones 1–60, revisiones independientes de los bloques A, C, D y E, contrato `tools/list` |
| `calibracion.md` | calibración de las puertas de fidelidad y de la puntuación de estilo |
| `benchmark.md` | modos A, B y C con agentes y sin ellos, hipótesis de tiempo y costo |

## Criterios de aceptación (§8.4)

| # | Criterio | Estado | Evidencia |
|---|---|---|---|
| 1 | **Calidad sin regresión.** En S, M y L, B ≥ A en IoU, topología y validez; 0 contraformas perdidas sin detectar en L y O; rasgos finos ≥ 1 px conservados o informados; irregularidad de O ≥ 80 % | Cumplido | `forma-bench/tests/regression.rs` (S, L y O en cada CI; M y M-hand cada noche), `baseline.json`, `quality::counterexamples`, `calibracion.md` §1 y §3 |
| 2 | **E y H del alfabeto privado sin intervención;** ninguna unión accidental | Cumplido (local) | `benchmark.md`, corrida B del alfabeto: la E con IoU 0,996 y la ranura conservada; la H agrupada [7, 8] con IoU 0,994 |
| 3 | **Síntesis:** todo sintetizado marcado y pendiente; lo aceptado ≥ 0,75; lo demás `needs_design` | Cumplido | `auto_policy_never_approves_synthesized`, `forma-synth/tests/synth.rs`, `calibracion.md` §2 |
| 4 | **Automatización:** C produce fuentes válidas sin IA; B sin coordenadas escritas por el agente | Cumplido | `mode_c_builds_a_valid_font_from_the_specimen`, la puerta de regresión, llamadas de las corridas B |
| 5 | **Consumo medido y publicado;** hipótesis de "< 2 min" y "< US$ 0,50" | Publicado | `benchmark.md`: "< 2 min" confirmada, "< US$ 0,50" refutada como meta general |
| 6 | **Disciplina:** registro y paridad, snapshot aditivo, migración v2 → v3, cobertura ≥ 80 %, CI en los 3 SO, puerta de calidad, umbrales calibrados | Ver abajo | |

Detalle del criterio 6:
- **Herramientas nuevas:** `preview_sheet`, `glyph_review`, `font_from_image`, `style_extract` y `glyph_synthesize` son comandos del registro con tests y escenario MCP en las dos eras del protocolo (`mcp_scenario_hito2_5.rs`).
- **Snapshot:** aditivo (`decisiones.md`, contrato del bloque E).
- **Migración v2 → v3:** testeada (`forma-store/tests/migration.rs`), con respaldo.
- **Cobertura:** (se anota al cerrar).
- **CI en los 3 SO:** pendiente del push, que decide el responsable.
- **Puerta de calidad:** `ci.yml` (`cargo test --workspace`) y `nightly.yml` (`bench-m`).
- **Umbrales:** `calibracion.md`.

## Decisiones pendientes del responsable

1. **Desviación de ajuste: ¿percentil 99 o máximo?** (decisión 48). Con el máximo, los trazados fieles caen al intento exacto por puntos de ruido, y los nodos se duplican o multiplican por seis. Hoy rige el p99 ≤ 0,5 px, con el máximo acotado a 2 px.
2. **Aprobar sintetizados con `glyph_review` bajo `auto`** (decisión 60): ¿exigir siempre un plan humano?
3. **Publicar las métricas agregadas del alfabeto privado** que están en `benchmark.md` (sin imágenes).
4. **Diacríticos para fuentes de bolas pesadas** (el alfabeto privado): la síntesis los rechaza por tamaño. Hoy el camino es dibujar ´ ¨ ˜ y trazarlos.
5. **Métrica `os2_metrics_match_hhea`** del perfil universal de fontspector (compilador, anterior a este hito).
