# Hito 2.5: benchmark A, B y C (Task 20)

Corridas del 2026-10-18 en la máquina del responsable (macOS 27, Apple silicon), con los binarios release de la rama `hito2-5`. Las transcripciones y los documentos quedan en `bench-out/` (ignorado por git). **Del alfabeto privado solo se publican métricas agregadas, sin imágenes**, y su publicación en el repositorio queda sujeta a la autorización del responsable.

- **Clientes:**
  - Claude Code 2.1.302 (modelo `claude-opus-5-5`);
  - Codex CLI 0.163.0 (modelo `gpt-6.1-sol`; su costo no se reporta porque va por suscripción).
- **Modo B:** servidor con `--profile image-font --output-schemas none`.
- **Instrucción:** idéntica a la del modo A (`tools/bench/prompts/mode_b.txt` = `mode_a.txt`), sin pistas de herramientas. Al agente lo guían solo las instrucciones y el catálogo del servidor.
- **Aprobación humana:** en una sesión sin interfaz, el cliente no puede responder una aprobación.
  - Claude Code no deja que el agente confirme por sí mismo los planes reservados a una persona (`plan_commit` → "MCPTool requires permission").
  - Un segundo cliente stdio no puede confirmar planes de otro proceso.
  - Por eso, la aprobación se modeló como **aprobación previa del operador** (`POLICY=auto`, contada como 1 intervención humana), salvo en S con Claude Code, que no la necesitó (8 glifos, sin cambios masivos).
  - La corrida de Inter con política `prompted` se detuvo correctamente: el agente se negó a esquivar el umbral trazando letra por letra. Queda registrada como `-blocked`.
- **Codex:**
  - aprueba las herramientas de este servidor con `mcp_servers.forma.default_tools_approval_mode="approve"`, para que las políticas sean las del propio servidor (sin tocar el sandbox);
  - se le cierra la entrada estándar (`</dev/null`), porque `codex exec` esperaba más entrada.

## Resultados con agente

### Claude Code

| Corrida | Política | Tiempo (s) | Turnos | Llamadas | Costo (USD) | Respuestas (bytes) | Motor (ms) | Resultado |
|---|---|---|---|---|---|---|---|---|
| **Línea base A**, specimen (transcripción del Hito 2) | auto | 153,1 | 43 | — | 1,34 | — | — | fuente completa |
| **Línea base A**, Alphabetical (transcripción del Hito 2) | auto | 986,4 | 173 | — | 6,94 | — | — | fuente completa |
| B specimen r1 | prompted | 58,9 | 14 | 13 | 0,688 | 8 069 | 311 | 8 letras, 0 errores, TTF y WOFF2 |
| B specimen r2 | prompted | 46,7 | 12 | 11 | 0,455 | 7 300 | — | ídem |
| B specimen r3 | prompted | 35,3 | 12 | 11 | 0,466 | 8 775 | — | ídem |
| **B specimen, media ± desviación** | | **47,0 ± 11,8** | **12,7 ± 1,2** | **11,7 ± 1,2** | **0,536 ± 0,131** | 8 048 ± 738 | | |
| B Inter 110 px (M) | prompted | 48,7 | 8 | 7 | 0,462 | 5 949 | — | detenida en la aprobación (correcto) |
| B Inter 110 px (M) | auto (1 intervención) | 76,5 | 22 | 21 | 0,938 | 30 374 | 1 034 | 26 letras, 0 errores, TTF y WOFF2 |
| B alfabeto privado | auto (1 intervención) | 68,9 | 19 | 18 | 0,772 | 28 363 | 5 843 | 26 letras + espacio; 29 sintetizados rechazados por el agente |

**Gasto total de las corridas con agente:** US$ 3,78 en Claude Code (el tope era US$ 15), más el uso de Codex por suscripción.

### Codex

| Corrida | Tiempo (s) | Turnos | Llamadas | Entrada | Caché leída | Salida | Respuestas (bytes) | Resultado |
|---|---|---|---|---|---|---|---|---|
| A specimen (herramientas granulares, catálogo completo) | 78,8 | 12 | 33 | 105 040 | 687 488 | 2 901 | 18 795 | fuente completa (10× `trace_compare`) |
| B specimen | 44,4 | 9 | 17 | 68 233 | 300 032 | 1 693 | 18 183 | fuente completa |

La corrida A de Codex usa el binario de esta rama con el catálogo completo y sin perfil, no el de `main`. El agente eligió las herramientas granulares, como en el Hito 2.

### Contra la línea base

| | Línea base A | B (Claude Code) | Mejora |
|---|---|---|---|
| specimen: tiempo | 153 s | 47 s (media) | 3,3× |
| specimen: turnos | 43 | 12,7 | 3,4× |
| specimen: costo | US$ 1,34 | US$ 0,54 | 2,5× |
| alfabeto: tiempo | 986 s | 69 s | 14,3× |
| alfabeto: turnos | 173 | 19 | 9,1× |
| alfabeto: costo | US$ 6,94 | US$ 0,77 | 9,0× |

Con Codex, de A a B: 1,8× menos tiempo, la mitad de llamadas y 44 % menos tokens de entrada más caché.

### Hipótesis

- **"< 2 min": confirmada en todas las corridas completas** (35–77 s). El motor tarda 0,3–5,8 s; el resto es el agente.
- **"< US$ 0,50": refutada como meta general.** Solo se cumple en el caso más pequeño (2 de 3 corridas del specimen: US$ 0,455 y 0,466). Inter (US$ 0,94) y el alfabeto (US$ 0,77) la superan. El costo lo dominan la caché leída (0,4–1,1 M tokens) y las imágenes que el agente pide para revisar, no las respuestas del servidor (8–30 KB). Es una mejora de 9× sobre la línea base, pero no la meta.

## Calidad (criterios de §8.4)

1. **Calidad sin regresión** (modo C, determinista; ver `decisiones.md` y la puerta `tests/regression.rs`):
   - **S, L y O:**
     - IoU contra la verdad 0,995, 0,998 y 0,983;
     - contrahuecos 8/8, 35/35 y 35/36 (el que falta está marcado para revisión);
     - 0 errores topológicos no detectados.
   - **M:**
     - 931/936 contrahuecos;
     - 24 glifos para revisión;
     - 2 "no detectados", el límite de medición conocido de la decisión 33.
   - **Irregularidad conservada en O:** 100 % de los dibujos irregulares sin tinta difuminada.
   - **Oráculos:** OTS, cmap y HarfBuzz pasan en los 17 casos de S, L y O; fontspector (perfil `opentype`) da 0 FAIL.
   - **Pendiente para después:** fontspector con su perfil universal marca `os2_metrics_match_hhea` (sTypoAscender 800 contra hhea 1000), una política de métricas del compilador anterior a este hito.
2. **E y H del alfabeto privado sin intervención** (corrida B):
   - la E con IoU 0,996 contra la tinta y la ranura conservada;
   - la H agrupada automáticamente de sus dos piezas, con IoU 0,994;
   - en S, M y L, ninguna unión accidental: la agrupación exige contraste de huecos y declara la ambigüedad.
3. **Síntesis:**
   - **Pendiente de revisión:** todo glifo sintetizado nace pendiente, con cualquier política.
   - **Bajo el umbral:** lo que no alcanza 0,75, o falla un componente o la estructura, sale como `needs_design`.
   - **Lo aceptado** en M puntúa ≥ 0,75 (calibración, sección 2).
   - **Alfabeto privado:** la autoprueba (0,85) marcó todos sus sintetizados para revisión. El agente los miró y rechazó los 29 creados, por ser más ligeros que las letras y por los 6 y 9 deformes. Así funciona el diseño: nada se aprueba solo, y la síntesis de estilos orgánicos queda como trabajo futuro (perfiles de engrosamiento y estrechamiento aprendidos de las letras).
4. **Automatización:**
   - el modo C produce fuentes válidas en S y M sin IA;
   - en modo B ningún agente escribió coordenadas: todo el contorno salió de `font_from_image`.
5. **Consumo:** publicado arriba.
6. **Disciplina:** ver `README.md`.
