# Capítulo 4 — Frontmatter OKF: los bytes son la verdad

Crate: [`crates/okf-core`](../crates/okf-core/src/lib.rs) ·
[referencia](https://pmaojo.github.io/okf-mcp/okf_core/)

Ya puedes convertir texto JSON en estructura. Pero los documentos de
memoria no son JSON: son Markdown con un encabezado YAML — el
frontmatter — donde viven `type`, `title`, `status` y `tags`, y un cuerpo
salpicado de enlaces `[[...]]` que forman el grafo. El servidor
necesita LEER esos metadatos y extraer esos enlaces.

Y aquí te topas con un muro que no esperabas: YAML. El formato
parece inocente — dos puntos, guiones, indentación — y es un
monstruo de especificación: anclas, referencias, bloques literales,
once formas de escribir un booleano. No vamos a implementar YAML
completo con `std`. Nadie debería, para esto.

La salida no es rendirse ni tragar: es un **subconjunto
documentado** que cubre el 95 % de los documentos de memoria reales
— escalares de una línea, listas en bloque, comentarios — con
rechazo explícito y localizado de todo lo demás. Y esa decisión
descansa sobre dos promesas; la primera gobierna el proyecto entero:

> **Los bytes originales del documento son la verdad.** El parser
> DERIVA metadatos; jamás regenera, normaliza ni "arregla" el
> documento. Lo que el agente escribió es lo que el siguiente agente
> leerá, byte a byte.

> **Lo que no entendemos, lo rechazamos con línea y motivo.** Un
> parser que acepta en silencio lo que no entiende corrompe datos
> con retraso, que es la peor forma de corromper datos.

La primera promesa ya es visible en el tipo: `OkfDocument` guarda
`body_offset: usize` — un OFFSET sobre el texto original — en lugar
de una copia del cuerpo. Quien quiera el cuerpo hace
`&raw[doc.body_offset..]`: cero copias, cero oportunidades de
divergencia.

## Tres funciones en cadena

```text
parse_document
  ├── split_frontmatter   →  (&str del YAML, body_offset)
  ├── parse_frontmatter   →  Vec<(String, FmValue)>
  └── scan_links          →  Vec<ConceptId>
```

**`split_frontmatter`** busca el cierre `---`, y la manera de
buscarlo esconde la primera trampa del capítulo. Un `raw.find("---")`
ingenuo encontraría los `---` DENTRO de un valor
(`title: uso de --- en medio`) — hay un test para eso. La versión
correcta recorre línea a línea con `split_inclusive('\n')`, que
conserva el `\n` en cada trozo: sumar longitudes de líneas da
offsets exactos sobre el original, sin contabilidad paralela.

**`parse_frontmatter`** procesa línea a línea con un estado mínimo:
`open_list: Option<String>` recuerda si la línea anterior abrió una
lista (`tags:`). Es una máquina de estados de dos estados — la forma
más simple de parser que existe — y basta porque el subconjunto
prohíbe anidamiento.

**`scan_links`** es una sola pasada sobre bytes, sin regex:

```rust
while i < bytes.len() {
    if at_line_start && bytes[i..].starts_with(b"```") {
        in_code_fence = !in_code_fence;
    }
    at_line_start = bytes[i] == b'\n';
    if !in_code_fence && bytes[i..].starts_with(b"[[") {
        // ...extraer hasta "]]", validar como ConceptId
    }
    i += 1;
}
```

Fíjate en `in_code_fence`: un ejemplo de código que contiene
`[[esto/no-cuenta]]` no debe crear una arista del grafo. El escáner
entiende justo la cantidad mínima de Markdown para no mentir.

Y el detalle que cierra el círculo con el capítulo 1: cada enlace
pasa por `ConceptId::parse`. Un documento con `[[../etc/passwd]]`
**no se guarda** — el error viaja con el offset del byte exacto. La
validación de la frontera protege también las aristas del grafo.

## El formateador que pierde escrituras

La versión rota de este capítulo es una tentación que has sentido si
alguna vez escribiste un linter: "ya que parseo el documento, lo
regenero limpio al guardar".

```rust
// ❌ NO HACER: parsear y regenerar el documento
pub fn guardar(doc: &OkfDocument) -> String {
    let mut out = String::from("---\n");
    out.push_str(&format!("type: {}\n", doc.doc_type));
    if let Some(t) = &doc.title {
        out.push_str(&format!("title: {t}\n"));
    }
    // ... reconstruir tags, extra, cuerpo ...
    out
}
```

Pásale este documento:

```yaml
---
type: person
# Revisar con legal antes de publicar
title: "Alice: directora"
tags: []
---
```

La regeneración pierde el comentario (los parsers no los conservan).
Convierte `"Alice: directora"` en `Alice: directora` sin comillas —
que ahora parsea DISTINTO: clave `title` con valor `Alice` y basura
detrás. Y reescribe `tags: []` en otro estilo. Nada de esto lo pidió
nadie.

Ahora encadena las consecuencias con lo que ya construiste: el hash
del capítulo 2 se calcula sobre los bytes. La regeneración cambió
los bytes, luego cambió el `ContentId`, sin que ningún humano ni
agente haya editado nada. El CAS del capítulo 6 verá un "conflicto"
fantasma y un agente perderá su escritura por culpa de un
formateador bienintencionado.

Por eso el invariante es *bytes exactos*: el frontmatter parseado es
un ÍNDICE del documento, como el índice de un libro. Nadie reimprime
el libro desde su índice.

## La política de rechazo, testeada

El test más importante del crate no comprueba lo que aceptamos, sino
lo que NOS NEGAMOS a aceptar:

```rust
#[test]
fn rechaza_yaml_fuera_del_subconjunto() {
    for raw in [
        "---\ntype: a\nnested:\n  key: valor\n---\n",   // anidamiento
        "---\ntype: a\ntexto: |\n  bloque\n---\n",       // bloque literal
        "---\ntype: a\ntype: b\n---\n",                  // clave duplicada
        "---\ntype: a\n- suelto\n---\n",                 // lista sin clave
    ] { ... }
}
```

Cada caso es una feature de YAML que no implementamos y que debe
fallar ruidosamente, no "funcionar más o menos". Completan la suite:
enlaces con traversal, presupuestos, y el caso del `---` en medio de
un valor.

## La frontera de producción

> 🧰 **La rueda de serie:** en producción, [`gray_matter`](https://docs.rs/gray_matter) para frontmatter y [`pulldown-cmark`](https://docs.rs/pulldown-cmark) para escanear Markdown (`serde_yaml` está archivado; mira sus sucesores). El mapa completo y el criterio para elegir: [La rueda de serie](la-rueda-de-serie.md).

El plan del proyecto (hito 2) añade `okf-yaml`: un adaptador con un
crate YAML maduro que valida el documento COMPLETO contra el formato
OKF real. La convivencia será:

- `okf-core` (este subconjunto): tests, herramientas locales, y la
  extracción de enlaces (que es nuestra, no de YAML).
- `okf-yaml`: la validación autoritativa en el camino de escritura
  de producción.

Lo que NO cambia en producción: la regla de bytes exactos. El crate
YAML solo LEE; el documento almacenado sigue siendo el original.

---

## Apéndice del capítulo

### Conceptos de Rust

* **Iteradores y `split_inclusive`:** procesar texto en Rust se hace con iteradores, flujos perezosos (lazy) que no trabajan hasta que se lo pides. `split_inclusive('\n')` divide en líneas pero, a diferencia de la mayoría de lenguajes, deja el `\n` al final de cada una: acumular longitudes de líneas da el offset exacto de cada carácter en el documento original.
* **Offsets frente a copias de datos:** en lugar de guardar una copia del cuerpo en `OkfDocument` (duplicando memoria en el heap), guardamos `body_offset: usize` (un número). El cuerpo se lee bajo demanda con un *slice* del original: `&raw[doc.body_offset..]`. Ahorra memoria y hace imposible la divergencia.
* **`Option` como máquina de estados:** `open_list: Option<String>` recuerda si la línea anterior abrió una lista. `Some(clave)` = leyendo elementos de esa lista; `None` = leyendo claves normales. `Option` sustituye a las variables centinela (strings vacíos, nulls) de forma segura.

### Memoria y asignación

- Los límites van ANTES del trabajo: `max_document_bytes` en la
  primera línea de `parse_document`, `max_frontmatter_bytes` DURANTE
  la búsqueda del cierre (no después de acumular 100 MB de
  "frontmatter" sin cerrar), `max_links_per_document` antes de hacer
  `push`.
- `split_frontmatter` devuelve `&str` prestados: cero copias hasta
  que un valor concreto se convierte en `String` del resultado.
- `scan_links` deduplica con `BTreeSet` (orden determinista otra
  vez) y su coste es O(n) sobre el cuerpo con una asignación por
  enlace único.

### SOLID en juego

- **S:** `okf-core` entiende el FORMATO. No almacena (capítulo 7),
  no decide conflictos (capítulo 6), no recorre grafos (capítulo 5).
  Cuando OKF publique la v0.2, esta es la única caja que se abre.
- **O:** `FmValue` es un enum con dos variantes (`Scalar`, `List`).
  Añadir `Number` o `Date` en el futuro = nueva variante + los
  `match` que el compilador señale. Extensión sin modificar la
  estructura del parser.
- **L (preparación):** `parse_document` y el futuro
  `ConformantOkfParser` deberán ser sustituibles en el camino de
  validación: mismo tipo de resultado, misma semántica de "los bytes
  no se tocan". El contrato ya está definido por este crate; el
  adaptador tendrá que cumplirlo, no al revés.
- **D:** ¿de quién depende `okf-core`? Solo de `memory-model` (los
  sustantivos). No sabe nada de JSON, MCP ni almacenes que viven
  "más arriba".

### Ejercicios

1. **Guiado.** Añade soporte para valores booleanos (`draft: true`)
   como `FmValue::Bool`. Empieza por los tests: ¿`True`, `TRUE`,
   `yes` son booleanos? (En YAML 1.1 sí, en 1.2 no — decide TÚ y
   documenta.)
2. **Medio.** El escáner de enlaces no entiende código inline
   (`` `[[x]]` `` con backticks simples). Arréglalo sin regex y sin
   romper el caso del fence. ¿Cuántos estados tiene ahora tu máquina?
3. **Abierto.** Escribe un *fuzzer* casero: genera 10 000 documentos
   aleatorios (bytes al azar, y también mutaciones de documentos
   válidos) y verifica que `parse_document` (a) nunca hace panic,
   (b) nunca acepta y luego falla al re-parsear su propio input.
   ¿Encontraste algo? (Los autores de este libro encontraron un bug
   de bucle infinito en SHA-256 con menos que eso; véase el
   capítulo 2.)

Siguiente: [Capítulo 5 — El grafo acotado](05-grafo-acotado.md).
