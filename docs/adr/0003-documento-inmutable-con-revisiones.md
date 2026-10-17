# ADR 0003: Documento inmutable con revisiones por entidad, concurrencia optimista e historial por snapshots con actores

- Estado: aceptada (Hito 1)
- Fecha: 2026-10-16
- Referencias: `docs/03-architecture.md` §5 y §6, `docs/07-mcp-ai-architecture.md` §5 y §6

## Contexto

Una persona y uno o varios agentes editan a la vez la misma fuente. Un agente nunca debe sobrescribir en silencio lo que la persona acaba de hacer, deshacer debe ser correcto por construcción, y leer, compilar o previsualizar no debe bloquear la edición.

## Decisión

- **Documento inmutable.** `Font` comparte estructura mediante `Arc` (`Arc<BTreeMap<GlyphId, Arc<Glyph>>>`) y se edita por copia al escribir (`Arc::make_mut`). Un `Document` guarda un `Arc<Font>` por revisión, así que leer una revisión es barato y seguro desde cualquier hilo. Se eligió `Arc` manual en lugar de colecciones persistentes (`im`/`rpds`): sin dependencias nuevas y suficiente para el modelo del Hito 1.
- **Revisiones.** `Rev(u64)`: un documento nuevo o recién abierto empieza en 0 y cada cambio de estado (commit, undo, redo, revert, restore) suma 1. Cada entidad (información de la fuente, métricas verticales de un master, cada glifo, el orden de glifos) recuerda la revisión y el actor de su último cambio.
- **Concurrencia optimista por entidad.** Las herramientas que cambian el documento llevan `base_rev`. Si alguna entidad que la transacción toca cambió después de `base_rev`, falla con `REVISION_CONFLICT` y lista todas las entidades en conflicto; los cambios en entidades distintas nunca entran en conflicto. Una transacción es todo o nada: el primer error se devuelve sin tocar nada. `GlyphOrder` queda registrado en `touched` pero **nunca provoca conflicto** (no hay comando de reordenar, y crear o borrar glifos distintos a la vez no debe chocar).
- **Historial lineal y global, por snapshots**, con autoría: cada entrada tiene `tx`, `rev`, `actor`, `label`, `touched` y `kind` (`commit`, `undo`, `redo`, `revert`, `restore`). Los actores se serializan como `user`, `cli` o `agent:<nombre del cliente>`.
  - `undo`/`redo` solo actúan sobre la última transacción **del mismo actor** (si no, el error sugiere `history_revert`); un commit nuevo vacía la pila de rehacer.
  - `revert(tx)` crea una revisión nueva que deshace esa transacción, si ninguna de sus entidades cambió después; vale para cualquier tipo de entrada.
  - `restore(rev, base_rev)` crea una revisión nueva con el estado de `rev`, con comprobación de conflictos por entidad (sin `GlyphOrder`); nunca borra historial y siempre exige aprobación (ADR 0005).
- `document_diff` compara dos revisiones cualesquiera.

## Alternativas

- **Comandos inversos para deshacer:** menos memoria, pero cada comando necesitaría un inverso correcto; los snapshots son más simples y obviamente correctos.
- **Colecciones persistentes `im`/`rpds`:** compartir estructura sin escribir `Arc` a mano, a cambio de una dependencia y un modelo menos transparente.
- **Una sola revisión global como token de conflicto:** más simple, pero cualquier edición de otro actor invalidaría la de todos, aunque tocara otro glifo.
- **Bloqueos pesimistas por documento o glifo:** impiden la edición simultánea y requieren liberar bloqueos si un agente se cuelga.

## Consecuencias

- La memoria crece con las revisiones (se comparte lo que no cambia, pero el historial completo se conserva); habrá que decidir una política de compactación cuando el historial largo pese.
- Los agentes deben releer tras un `REVISION_CONFLICT`; la guía de conexión (`docs/guides/connect-agents.md`) lo explica.
- Un futuro comando de reordenar glifos tendrá que aportar su propia clave de conflicto.
- En la interfaz, Cmd/Ctrl+Z podrá deshacer la última transacción de cualquier actor (decisión de producto para el Hito 3); por MCP rige la regla del mismo actor.
