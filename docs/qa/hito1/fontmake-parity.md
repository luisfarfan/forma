# Paridad de compilación con fontmake (Hito 1, criterio 8)

> Ejecución local: macOS (arm64), 2026-10-16, `scripts/acceptance.sh`, paso 4.
> Herramientas: fontc 1.0.0 (dentro de `forma-compile`); fontmake 3.12.2 con ufo2ft 3.9.1 y fontTools 4.67.0; Python 3.13.15.
> Los dos compiladores corren con `SOURCE_DATE_EPOCH=0`.

## Qué se compara

`tools/oracle/verify_font.py --fontmake-parity` toma dos fuentes y las compara:

- el TTF que compila FORMA con fontc, el que el agente exporta en el escenario MCP;
- el que fontmake genera a partir del UFO de ese mismo documento, con `fontmake -u forma-demo.ufo -o ttf --output-path …` (el UFO sale de `forma export … --format ufo`).

fontc y fontmake no tienen por qué convertir las cúbicas en cuadráticas igual, así que la paridad no exige coincidencia punto a punto. Se exige, glifo a glifo:

| Criterio | Tolerancia |
|---|---|
| Mismo conjunto de glifos y mismo `cmap` | exacta |
| Avance (`hmtx`) | exacta |
| Número de contornos | exacta |
| Caja envolvente exacta de las curvas (`BoundsPen`) | ±1 unidad por coordenada |
| Área absoluta del contorno (`AreaPen`) | ±0,5 % |

Las demás diferencias **se listan y no hacen fallar** la comprobación. Están en la sección siguiente.

## Resultado: PASS

Hay 12 glifos. La diferencia máxima de caja es de 0 unidades y la de área, del 0,0000 %.

| Glifo | Avance | Contornos | Puntos FORMA / fontmake | Δ caja | Área (unidades²) | Δ área |
|---|---:|---:|---:|---:|---:|---:|
| .notdef | 500 | 2 | 8 / 8 | 0 | 100000,0 | 0 % |
| space | 250 | 0 | 0 / 0 | 0 | 0 | 0 % |
| A | 680 | 2 | 11 / 11 | 0 | 155740,0 | 0 % |
| B | 640 | 3 | 34 / 36 | 0 | 177158,8 | 0 % |
| C | 680 | 1 | 30 / 32 | 0 | 113355,7 | 0 % |
| H | 700 | 1 | 12 / 12 | 0 | 142400,0 | 0 % |
| O | 760 | 2 | 28 / 36 | 0 | 150400,5 | 0 % |
| a | 560 | 2 | 30 / 35 | 0 | 124529,9 | 0 % |
| b | 560 | 2 | 30 / 35 | 0 | 143798,0 | 0 % |
| c | 540 | 1 | 28 / 30 | 0 | 78987,3 | 0 % |
| n | 580 | 1 | 20 / 22 | 0 | 96205,5 | 0 % |
| o | 560 | 2 | 24 / 32 | 0 | 101111,4 | 0 % |

## Diferencias fuera de la paridad (listadas, no fallan)

| Diferencia | FORMA (fontc) | fontmake | Explicación |
|---|---|---|---|
| Puntos de `glyf` en 8 glifos (B C O a b c n o) | 20–34 | 22–36 | fontc (write-fonts) omite el on-curve que es el punto medio de sus dos off-curve vecinos, antes o después de redondear. Es un punto implícito. fontmake (ufo2ft) conserva esos puntos en las fuentes estáticas. Si se escriben los puntos implícitos, los contornos coinciden punto a punto, salvo el orden de los contornos y el punto inicial. La desviación máxima es de **0,5 unidades**, en C O a b c n o, y de 0 en B. El 0,5 sale de cómo cada compilador guarda ese nodo: el punto medio implícito de fontc cae en media unidad, y fontmake guarda el nodo redondeado. La caja y el área coinciden. |
| Punto inicial de los contornos (p. ej. `o`) | (219, −10), off-curve | (280, −10), on-curve | Es consecuencia de la fila anterior: fontc omitió el on-curve inicial porque era implícito. |
| Orden de los contornos de `B` | exterior, contraforma inferior, contraforma superior (orden del UFO) | exterior, superior, inferior | fontmake elimina solapamientos por defecto (`RemoveOverlapsFilter`, booleanOperations), y ese filtro reescribe los contornos. Con `--keep-overlaps`, el orden es el de fontc. fontc no elimina solapamientos (riesgo T1). |
| `maxp.maxPoints` | 34 | 36 | Es consecuencia de los puntos implícitos (el máximo está en B). |
| `name` ID 5 (versión) | `Version 1.000;fontc 1.0.0` | `Version 1.000` | fontc añade su nombre y su versión a la cadena de versión. |
| `head.checkSumAdjustment` | 933823511 | 550205991 | Es la suma de control de todo el archivo, así que cambia con cualquier byte. |

Las demás tablas coinciden: `head` (con `created` y `modified`, gracias a `SOURCE_DATE_EPOCH=0`), `hhea`, `OS/2` y `post`. Las dos fuentes tienen el mismo juego de tablas: `OS/2 cmap glyf head hhea hmtx loca maxp name post`. Ninguna tiene instrucciones ni la marca `OVERLAP_SIMPLE`.

## Límite conocido

La paridad se cumple porque el fixture (`tests/fixtures/hito1/`) no tiene contornos solapados. Con un glifo solapado, fontmake uniría los contornos y fontc no, de modo que el número de contornos y el área diferirían. Cuando FORMA admita solapamientos y su eliminación (fase 4), esta comparación tendrá que compilar con `--keep-overlaps` o eliminar antes los solapamientos en FORMA.
