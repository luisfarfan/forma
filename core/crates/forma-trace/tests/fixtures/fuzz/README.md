# Entradas del fuzzer

Entradas que el fuzzer nocturno (`core/fuzz`, objetivo `image_decode`) encontró problemáticas. Son bytes generados por libFuzzer a partir de nuestro propio corpus: no contienen material de terceros.

- `dust-column.jpg` (2026-10-19): JPEG de 16 × 57 616 px. Cada fila guarda hasta 129 485 manchas, y la fusión por pares de una fila tardaba más de 10 s bajo los sanitizadores. Test: `analyze.rs::a_column_of_dust_is_analyzed_quickly`.
