# Capítulo 18 — Razonamiento ligero: triples y OWL-RL acotado

Crate: [`crates/ontology-core`](../crates/ontology-core/src/lib.rs) ·
[referencia](https://pmaojo.github.io/okf-mcp/ontology_core/)

> **Capítulo opcional**, en la misma categoría que el 15: nada del
> núcleo depende de este crate. `okf-core` sigue sin saber que existe
> — `parse_document` es, como siempre, la única fuente de verdad
> sobre qué es un documento válido. Si no te interesa razonar sobre
> el grafo, puedes saltarte este capítulo entero.

## 1. El problema — y por qué el capítulo 15 dijo que no

El [capítulo 15, sección 8](15-mcp-apps-visualizaciones.md#8-frontera-de-producción)
fue explícito: este proyecto **no** adopta OWL/RDF para tipar
`doc_type` ni los enlaces `[[...]]`. La razón que dio entonces sigue
siendo correcta como se escribió: OWL trae clases, subclases,
propiedades tipadas y, para sacarle partido, un razonador de
subsunción — maquinaria pensada para bases de conocimiento
compartidas a gran escala. Añadir ese andamiaje sin un caso de uso
real habría sido la abstracción prematura que el tutorial lleva
evitando desde el capítulo 3.

El caso de uso real llegó: un agente que ya tiene `type: person` y
`[[reports_to:people/bob]]` en el frontmatter (capítulo 4) quiere
preguntar "¿A quién de mi equipo le informa Alice, transitivamente?"
o "si `person` es sub-clase de `agent`, ¿Alice cuenta como `agent`
para esta consulta?" — preguntas que hoy exigen que el AGENTE
recorra el JSON a mano y reconstruya la lógica en su cabeza, cada
vez, sin garantía de acertar en los casos con ciclos o cadenas
largas.

La tentación es la misma que rechazó el capítulo 15, solo que ahora
con nombre concreto: `oxigraph`, un almacén RDF + SPARQL completo.
Y la razón para seguir sin él también es la misma de siempre —
`scripts/check-std-only.sh` prohíbe dependencias externas en el
núcleo, y "cargar un motor SPARQL para deducir que `person` es un
`agent`" es una desproporción de la misma familia que "cargar el
grafo entero" lo fue en el capítulo 5.

## 2. El invariante

> **`materialize` nunca excede su presupuesto — iteraciones, triples
> totales — y SIEMPRE informa de si truncó y por qué.**

Es literalmente el invariante del capítulo 5 (`bounded_bfs`),
trasplantado a un algoritmo distinto. No es casualidad: un
recorrido de grafo y un punto fijo de reglas de inferencia comparten
el mismo peligro — ambos pueden no terminar nunca sobre datos
cíclicos — y la misma solución: acotar y declarar el corte, no
prometer una respuesta completa que a veces sería mentira.

## 3. La implementación mínima

Primero, nombrar lo que ya existía. Un documento OKF parseado YA es
casi un conjunto de triples — el capítulo 4 lo extrajo sin llamarlo
así:

```rust
pub enum Object {
    Concept(ConceptId),
    Literal(String),
}
pub struct Triple {
    pub subject: ConceptId,
    pub predicate: String,
    pub object: Object,
}

pub fn triples_from_document(subject: &ConceptId, doc: &OkfDocument) -> Vec<Triple> {
    let mut out = Vec::with_capacity(1 + doc.tags.len() + doc.links.len());
    out.push(Triple { subject: subject.clone(), predicate: RDF_TYPE.to_string(),
                       object: Object::Literal(doc.doc_type.clone()) });
    for tag in &doc.tags { /* (sujeto, "tag", tag) */ }
    for link in &doc.links {
        // predicado tipado si el enlace lo trae ([[rel:destino]]),
        // "related" si no ([[destino]] a secas)
    }
    out
}
```

`type: person` se convierte en `(sujeto, rdf:type, "person")`;
`[[reports_to:people/bob]]` en `(sujeto, "reports_to",
people/bob)`. Cero parsing nuevo: es una re-etiqueta de lo que
`okf_core::parse_document` ya produjo.

Segundo, la ontología — declarada a mano en Rust, sin parser
OWL/YAML de por medio, igual que el resto del núcleo declara sus
invariantes en el sistema de tipos:

```rust
pub struct SubClassOf { pub subclass: String, pub superclass: String }
pub enum PropertyAxiom {
    SubPropertyOf { sub: String, sup: String },
    Transitive(String),
    Symmetric(String),
    InverseOf { property: String, inverse: String },
}
pub struct Ontology { pub classes: Vec<SubClassOf>, pub properties: Vec<PropertyAxiom> }
```

Y el motor: encadenamiento hacia adelante hasta punto fijo, acotado
por `ReasoningBudget { max_iterations, max_triples }`:

```rust
let mut known: BTreeSet<Triple> = /* los hechos de partida */;
while !truncated_by_triples && iterations_used < budget.max_iterations {
    iterations_used += 1;
    let derived = apply_rules_once(&known, ontology);
    let before = known.len();
    for triple in derived {
        if known.len() >= budget.max_triples { truncated_by_triples = true; break; }
        known.insert(triple);
    }
    if known.len() == before { break; } // punto fijo: nada nuevo esta ronda
    if iterations_used == budget.max_iterations { truncated_by_iterations = true; }
}
```

`apply_rules_once` es cinco reglas, cada una una traducción directa
de su axioma: `subClassOf` propaga `rdf:type` hacia arriba,
`SubPropertyOf` copia el hecho bajo el predicado general,
`Transitive` cierra pares de aristas que comparten extremo,
`Symmetric` e `InverseOf` generan el hecho espejo. Ninguna regla
sabe de las otras — el punto fijo las combina por repetición, no por
orquestación explícita.

## 3.5. Conceptos de Rust en este capítulo

* **`BTreeSet<Triple>` como el propio mecanismo de deduplicación:**
  `apply_rules_once` no comprueba si un triple ya es conocido antes
  de generarlo — genera candidatos libremente y dependen de
  `known.insert()` (que devuelve `false` si ya estaba) para
  descartar duplicados. El mismo truco que `visited_set.insert()` en
  el capítulo 5, aplicado a hechos en vez de a nodos. Por eso `Triple`
  y `Object` derivan `Ord`: sin orden total no hay `BTreeSet`.
* **`let Object::Literal(class) = &triple.object else { continue }`:**
  el patrón `let-else` de Rust 1.65+. Sin él, filtrar-y-extraer exige
  un `match` de dos brazos donde uno solo hace `continue`; `let-else`
  dice "si no encaja, sal de aquí" en una línea, sin anidar el resto
  de la función dentro del brazo que sí importa.
* **`filter_map` para la regla transitiva:** de los triples conocidos
  con predicado `P`, solo los que apuntan a OTRO concepto (no a un
  literal) pueden encadenarse; `filter_map` descarta los literales y
  extrae `(&sujeto, &objeto)` en el mismo paso, sin un `filter`
  seguido de un `map`.

## 4. Una versión que sí colgaría

```rust
// ❌ NO HACER: sin presupuesto, un ciclo de subClassOf no termina nunca.
fn materialize_ingenuo(facts: &[Triple], ontology: &Ontology) -> Vec<Triple> {
    let mut known: BTreeSet<Triple> = facts.iter().cloned().collect();
    loop {
        let derived = apply_rules_once(&known, ontology);
        let before = known.len();
        known.extend(derived);
        if known.len() == before { break; } // ← la única red de seguridad
    }
    known.into_iter().collect()
}
```

Parece razonable: "itero hasta que no cambie nada, ¿qué puede
salir mal?" Con una ontología acíclica, nada — es exactamente lo que
hace `materialize` cuando el presupuesto nunca se agota. El problema
es la ontología que SÍ tiene un ciclo, y una ontología declarada a
mano por una persona con prisa (`a subClassOf b`, y más tarde,
sin recordar la primera línea, `b subClassOf a`) es fácil de escribir
sin querer.

## 5. Por qué falla

Con el ciclo `a→b→a`, cada ronda de `apply_rules_once` sigue
produciendo `(x, rdf:type, a)` y `(x, rdf:type, b)` — ambos YA están
en `known`, así que `known.insert()` los descarta y `known.len()` no
crece. El bucle SÍ termina (el test
`ciclos_de_subclass_of_no_cuelgan_el_motor` lo prueba), pero por
suerte estructural, no por diseño: el corte real es que la regla
`subClassOf` solo puede generar triples de la forma `(x, rdf:type,
C)` con `C` tomado de un conjunto finito de clases declaradas, así
que el punto fijo siempre llega. Una regla con más grados de
libertad — imagina una que generara un concepto NUEVO en vez de
reusar uno existente — no tendría esa suerte, y `materialize_ingenuo`
correría para siempre. `max_iterations` es la diferencia entre
"este caso particular no cuelga" y "ningún caso puede colgar": el
test `respeta_presupuesto_de_iteraciones` fuerza una cadena de 5
`subClassOf` con presupuesto de 1 ronda y comprueba que el resultado
queda A MEDIAS, marcado como tal — el comportamiento correcto ante
una ontología más profunda de lo que el presupuesto anticipó.

## 6. Presupuesto, no optimización prematura

La regla `Transitive` compara cada arista con cada arista
(`O(aristas²)` por ronda) para encontrar pares que comparten
extremo. Con un índice por nodo de origen sería más rápido — y sería
exactamente la clase de optimización sin medir que este tutorial ha
evitado desde el capítulo 2. `max_triples` ya acota el peor caso
(nunca hay más de `budget.max_triples` aristas que comparar), y el
caso de uso real — la vecindad acotada de un solo documento,
capítulo 5 — nunca se acerca a ese límite. Optimizar el bucle
interno antes de que un perfil real lo pida sería resolver un
problema que todavía no existe.

## 7. Tests

Nueve tests unitarios cubren cada regla por separado
(`subclass_of_es_transitivo_por_encadenamiento`,
`propiedad_transitiva_deriva_la_cadena_completa`,
`propiedad_simetrica_deriva_el_par_inverso`,
`inverse_of_deriva_el_predicado_contrario`,
`sub_property_of_propaga_hacia_la_propiedad_general`), los dos
cortes de presupuesto por separado
(`respeta_presupuesto_de_triples`,
`respeta_presupuesto_de_iteraciones` — con la cadena de 5
`subClassOf` de la sección 5) y el caso patológico que casi cuelga
el motor (`ciclos_de_subclass_of_no_cuelgan_el_motor`). Los dos
doctests (`triples_from_document`, `materialize`) son ejemplos
mínimos que compilan y pasan en cada `cargo test --doc`, como todo
lo demás desde el capítulo 16 — y son el primer sitio donde alguien
que no ha leído este capítulo puede ver la API en diez líneas.

## 8. Frontera de producción

> 🧰 **La rueda de serie:** para razonamiento real a escala —
> múltiples ontologías que se cruzan, SPARQL, persistencia nativa de
> grafos RDF —
> [`oxigraph`](https://github.com/oxigraph/oxigraph); para Datalog
> genérico con reglas más expresivas que las cinco fijas de este
> capítulo, [`crepe`](https://docs.rs/crepe) o
> [`ascent`](https://docs.rs/ascent). El mapa completo y el criterio
> para elegir: [La rueda de serie](la-rueda-de-serie.md).

Este crate resuelve razonamiento EN MEMORIA sobre los triples de un
recorrido ya acotado (capítulo 5). Persistir los triples derivados —
para no re-materializar la clausura en cada lectura — es trabajo de
un adaptador, no de `ontology-core`: una tabla `triples(subject,
predicate, object, derived)` en `supabase-store` (capítulo 12), con
las consultas de subsunción simple servidas por una CTE recursiva de
Postgres (`WITH RECURSIVE ancestros AS (...)`) en vez de volver a
correr `materialize` por cada lectura — el mismo patrón de "el
algoritmo no conoce al almacén" del capítulo 5, `NeighborSource`
otra vez, con otro nombre.

## 9. Principios SOLID en juego

* **S:** `ontology-core` solo razona. No parsea documentos (eso es
  `okf-core`), no recorre el grafo (eso es `graph-core`), no
  persiste nada. `triples_from_document` es la única costura con
  `okf-core`, y es una función pura de traducción, no una dependencia
  bidireccional.
* **D:** la dirección de la flecha es la de siempre — `ontology-core`
  depende de `memory-model`/`okf-core` (tipos de dominio), nunca al
  revés. Un futuro adaptador de persistencia dependerá DE
  `ontology-core`, no lo contrario.
* **O, con una grieta honesta:** añadir una consulta nueva (por
  ejemplo, "¿son estos dos conceptos `owl:sameAs`?") es abrir
  `Ontology` y `PropertyAxiom` con una variante más — no toca
  `materialize`. Pero añadir esa variante SÍ exige un brazo nuevo en
  el `match` de `apply_rules_once`: el conjunto de reglas no está
  cerrado a modificación de verdad, está cerrado a modificación DEL
  BUCLE DE PUNTO FIJO, que es la parte que de verdad puede esconder
  un bug de terminación. Fingir que las cinco reglas son extensibles
  sin tocar código sería la clase de mentira arquitectónica que este
  tutorial intenta no contarse.

## 10. ¿Por qué esta vez sí, si el capítulo 15 dijo que no?

Vale la pena decirlo explícito, porque la respuesta corta ("llegó un
caso de uso real") suena a excusa que serviría para justificar
cualquier cosa. Lo que hace que esto NO sea la abstracción prematura
que el capítulo 15 evitó:

- **`doc_type` sigue siendo un string libre.** `okf-core` no cambió
  una línea; nada obliga a nadie a declarar una `Ontology` para que
  `parse_document` siga funcionando exactamente igual que en el
  capítulo 4.
- **Sin `Ontology` declarada, `materialize` es un no-op literal**
  (`ontology.classes` y `.properties` vacíos ⇒ `apply_rules_once`
  devuelve `Vec::new()` ⇒ el punto fijo se alcanza en la primera
  ronda). Razonar es estrictamente opt-in.
- **Cero dependencias externas, presupuesto explícito.** Es la misma
  disciplina que rige `graph-core` desde el capítulo 5, aplicada a un
  problema distinto — no "adoptar OWL", sino tomar prestada la idea
  de subsunción con la forma que ya tiene el resto de este núcleo.

## 12. Actualización — `memory_reason` y ontologías por referencia

El ejercicio 3 de más abajo preguntaba por el esquema de persistencia
y la herramienta MCP: los dos existen ya. `store_core::TripleStore`
(`save_triples`/`load_triples`, mismo patrón de error fijo que
`MemoryRepository`) tiene una implementación por backend —
`InMemoryStore` con un `BTreeMap`, `SupabaseStore` con una tabla
`triples` reemplazada transaccionalmente por sujeto, `IndexedStore`
delegando en el lado Supabase (los triples son un índice de lectura,
no la fuente de verdad de GitHub), `GithubStore` aceptando y
descartando el guardado (git no tiene índice de consultas propio,
igual que su `NeighborSource` ya degrada sin fallar). `memory_reason`
reúne los hechos del vecindario acotado de un `concept_id` (el mismo
recorrido que `memory_resolve`), aplica `materialize` y persiste la
clausura salvo `persist: false`.

Lo que NO tenía la primera versión de la herramienta era una forma de
declarar una `Ontology` sin volver a escribirla en cada llamada. La
solución: una `Ontology` es también, en sí misma, un documento OKF
normal — `type: ontology`, sin campos nuevos en el frontmatter, con
los axiomas en el CUERPO en el mismo formato de texto plano de la
sección 3:

```text
subclass_of: student -> person
transitive: depends_on
symmetric: married_to
sub_property_of: depends_on -> related
inverse_of: manages -> managed_by
```

`ontology_core::parse_ontology_document` lo parsea línea a línea —
cinco palabras clave, sin YAML ni JSON, con el mismo criterio de
"rechaza con línea y motivo" que el resto del núcleo
(`OntologyDocError::Malformed`/`InvalidAxiom`/`UnknownKeyword`, todos
con número de línea). `memory_reason` acepta un `ontology_id`
opcional: si se da, `MemoryTools::load_ontology_document` lee ESE
documento (exige `type: ontology`; cualquier otro tipo es un error
legible por el modelo, no un triple vacío en silencio), lo parsea, y
sus axiomas se SUMAN a los `classes`/`properties` inline — ninguno
reemplaza al otro, así que "usa el vocabulario del equipo más una
excepción de esta llamada" es una sola petición, no dos.

La pieza que lo cierra es `ToolHandler::instructions()` (capítulo 15
extendido — mismo Abierto/Cerrado que `ui_resources()`, cuerpo por
defecto `None`): el texto que el servidor manda en `initialize`
le dice al agente, antes de que llame a ninguna herramienta, que
busque un documento `type: ontology` existente (`memory_search`) y
pase su `concept_id` como `ontology_id` en vez de inventar axiomas
sesión a sesión. Sin esto, "guardar la ontología como documento" es
una convención que solo funciona si alguien se acuerda de seguirla;
con esto, es el primer texto que cualquier cliente MCP real le
muestra al modelo.

Por qué el cuerpo y no el frontmatter, otra vez: el subconjunto YAML
de `okf-core` (capítulo 4) no soporta listas de objetos anidados —
`classes: [{subclass: a, superclass: b}]` está fuera del subconjunto
a propósito. Poner los axiomas en el cuerpo, con su propio parser
pequeño y determinista, es la misma decisión que ya tomó el resto de
este capítulo: un formato mínimo hecho a medida en vez de forzar el
problema dentro de una sintaxis que no lo soporta.

## 13. Ejercicios

1. ~~**Guiado.** El capítulo 15, ejercicio 3, preguntaba por una
   alternativa ligera a tipar `[[wiki-links]]` con OWL...~~
   Resuelto en la sección 3: `ontology-core` reutiliza
   `[[rel:destino]]` sin cambiar `okf-core::scan_links` — la relación
   tipada YA estaba ahí desde el capítulo 4, este capítulo solo la
   nombra explícitamente como predicado de un triple. Como ejercicio
   real: escribe la `Ontology` que declara `depends_on` como
   sub-propiedad de `related` y `manages` como inversa de
   `managed_by`, y verifica con un test que `triples_from_document` +
   `materialize` sobre un documento con `[[depends_on:...]]` produce
   el triple derivado esperado.
2. **Medio.** `apply_rules_once` es `O(aristas²)` para la regla
   transitiva. Diseña una versión con un índice
   `HashMap<ConceptId, Vec<ConceptId>>` construido una vez por ronda
   que la baje a casi lineal. ¿Sigue siendo determinista el ORDEN de
   los triples derivados? (pista: `known` es un `BTreeSet` — ¿importa
   el orden en que `apply_rules_once` los genera?)
3. ~~**Abierto.** Diseña el esquema de `triples` en `supabase-store` y
   la herramienta MCP `memory_reason` que lo sirva...~~ Resuelto en la
   sección 12 — salvo la mitad que sigue abierta A PROPÓSITO: hoy
   `memory_delete` (capítulo 17) no toca la tabla `triples` para nada.
   Si `people/bob` se borra lógicamente, los triples que otros
   conceptos derivaron sobre él (p. ej. `alice reports_to bob`) siguen
   ahí hasta que alguien vuelva a llamar `memory_reason` sobre esos
   sujetos. Diseña la limpieza: ¿`memory_delete` dispara un
   recálculo de todo lo que dependía del concepto borrado (¿cómo lo
   encuentras — un índice inverso por `object_value`?), o se deja
   como está documentado aquí y punto, aceptando que `triples` es una
   caché que puede quedar desactualizada hasta la siguiente
   materialización? ¿Cuál de las dos respuestas es más coherente con
   cómo `memory_delete` ya trata `links` (capítulo 17: "un borrado
   deja de enlazar")?
