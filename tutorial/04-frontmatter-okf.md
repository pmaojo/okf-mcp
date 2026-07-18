# Capítulo 4 — Frontmatter OKF: los bytes son la verdad

Crate: [`crates/okf-core`](../crates/okf-core/src/lib.rs)

## 1. El problema

Cada documento de memoria es Markdown con frontmatter YAML. El
servidor necesita LEER esos metadatos (`type`, `title`, `tags`) y
extraer los enlaces `[[...]]` para el grafo. Pero YAML completo es un
formato enorme (anclas, referencias, once formas de escribir un
booleano) y no vamos a implementarlo con `std`, ni deberíamos.

La decisión: un **subconjunto documentado** que cubre el 95 % de los
documentos de memoria reales, con rechazo explícito y localizado de
todo lo demás.

## 2. El invariante

Dos, y el primero gobierna todo el proyecto:

> **Los bytes originales del documento son la verdad.** El parser
> DERIVA metadatos; jamás regenera, normaliza ni "arregla" el
> documento. Lo que el agente escribió es lo que el siguiente agente
> leerá, byte a byte.

> **Lo que no entendemos, lo rechazamos con línea y motivo.** Un
> parser que acepta en silencio lo que no entiende corrompe datos
> con retraso, que es la peor forma de corromper datos.

El primer invariante explica una decisión visible en el tipo:
`OkfDocument` guarda `body_offset: usize` — un OFFSET sobre el texto
original — en lugar de una copia del cuerpo. Quien quiera el cuerpo
hace `&raw[doc.body_offset..]`: cero copias, cero oportunidades de
divergencia.

## 3. La implementación mínima

Tres funciones en cadena:

```text
parse_document
  ├── split_frontmatter   →  (&str del YAML, body_offset)
  ├── parse_frontmatter   →  Vec<(String, FmValue)>
  └── scan_links          →  Vec<ConceptId>
```

**`split_frontmatter`** busca el cierre `---` línea a línea con
`split_inclusive('\n')` — que conserva el `\n` en cada trozo, de
modo que sumar longitudes de líneas da offsets exactos sobre el
original. Un `raw.find("---")` ingenuo encontraría los `---` DENTRO
de un valor (`title: uso de --- en medio`); hay un test para eso.

**`parse_frontmatter`** procesa línea a línea con un pequeño estado:
`open_list: Option<String>` recuerda si la línea anterior abrió una
lista (`tags:`). Es una máquina de estados de dos estados — la forma
más simple de parser que existe — y es suficiente porque el
subconjunto prohíbe anidamiento.

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

Y el detalle que conecta con el capítulo 1: cada enlace pasa por
`ConceptId::parse`. Un documento con `[[../etc/passwd]]` **no se guarda** —
el error viaja con el offset del byte exacto.

## 3.5. Conceptos de Rust en este capítulo

En este capítulo vemos cómo trabajar con texto de forma eficiente y estructurar estados simples:

* **Iteradores y `split_inclusive`:** En Rust, procesar texto se hace a través de iteradores, que son flujos de datos perezosos (lazy) que no procesan nada hasta que se lo pides. El método `split_inclusive('\n')` divide el texto en líneas, pero a diferencia de la mayoría de lenguajes, deja el carácter `\n` al final de cada línea. Esto nos permite acumular de forma exacta las longitudes de las líneas procesadas para saber el offset (la posición en bytes) de cada carácter en el documento original.
* **Uso de Offsets frente a Copias de Datos:** En lugar de guardar una copia del cuerpo del texto en `OkfDocument` (lo cual implicaría duplicar la memoria en el heap), guardamos `body_offset: usize` (un simple número). El cuerpo del documento se lee "bajo demanda" haciendo un rebanado o *slice* del texto original: `&raw[doc.body_offset..]`. Esto no solo ahorra memoria, sino que asegura que no haya divergencia de datos.
* **`Option` como máquina de estados:** Al parsear el frontmatter, usamos `open_list: Option<String>` para recordar si la línea anterior abrió una lista (como `tags:`). Si es `Some(nombre_clave)`, sabemos que estamos leyendo elementos de esa lista; si es `None`, estamos leyendo claves normales. En Rust, `Option` sustituye la necesidad de variables "centinela" (como strings vacíos o valores nulos) de forma segura.

## 4. Una versión deliberadamente rota

La tentación de "normalizar al guardar":

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

## 5. Por qué falla

Con este documento de entrada:

```yaml
---
type: person
# Revisar con legal antes de publicar
title: "Alice: directora"
tags: []
---
```

la regeneración pierde el comentario (los parsers no los conservan),
cambia `"Alice: directora"` por `Alice: directora` (¡que ahora parsea
distinto: clave `title` con valor `Alice` y basura!), y reescribe
`tags: []` en otro estilo. El hash del contenido cambia sin que
ningún humano ni agente haya editado nada → el CAS del capítulo 6
detecta un "conflicto" fantasma → un agente pierde su escritura por
culpa de un formateador.

Por eso el invariante es *bytes exactos*: el frontmatter parseado es
un ÍNDICE del documento, como el índice de un libro. Nadie reimprime
el libro desde su índice.

## 6. Memoria y asignación

- Los límites van ANTES del trabajo: `max_document_bytes` en la
  primera línea de `parse_document`, `max_frontmatter_bytes` DURANTE
  la búsqueda del cierre (no después de acumular 100 MB de "frontmatter"
  sin cerrar), `max_links_per_document` antes de hacer `push`.
- `split_frontmatter` devuelve `&str` prestados: cero copias hasta
  que un valor concreto se convierte en `String` del resultado.
- `scan_links` deduplica con `BTreeSet` (orden determinista otra
  vez) y su coste es O(n) sobre el cuerpo con una asignación por
  enlace único.

## 7. Tests

El más importante es el que fija la política de rechazo:

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

Cada caso es una feature de YAML que NO implementamos y que debe
fallar ruidosamente, no "funcionar más o menos". Completan la suite:
enlaces con traversal, presupuestos, y el caso del `---` en medio de
un valor.

## 8. Frontera de producción

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

## 9. Principios SOLID en juego

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

## 10. Ejercicios

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
   ¿Encontraste algo? (Los autores de este tutorial encontraron un
   bug de bucle infinito en SHA-256 con menos que eso; véase el
   capítulo 2 del historial de git.)

Siguiente: [Capítulo 5 — El grafo acotado](05-grafo-acotado.md).
