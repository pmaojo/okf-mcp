# Capítulo 12b — El adaptador de GitHub: Persistencia como código

Crate: [`crates/github-store`](../crates/github-store)

## 1. El problema

¿Y si no tienes o no quieres desplegar PostgreSQL? Para la mayoría de bases de conocimiento, un repositorio de código ya tiene todo lo que necesitamos: versionado, prevención de pisadas (Compare-and-Swap nativo) y una API REST bien documentada. GitHub es el estándar de facto.

Podemos usar GitHub como fuente de verdad de la memoria, pero el adaptador se enfrenta a retos únicos. ¿Cómo representamos documentos, CAS y la historia completa de memoria en un repositorio Git? Y, más importante aún, ¿cómo mantenemos las promesas del contrato de Liskov cuando las búsquedas en red son lentas y no hay índices semánticos nativos en la API?

Aquí el conflicto proviene de la distancia: GitHub provee una API de escritura y otra de lectura, pero reconstruir el estado para buscar requiere descargar todo o mantener un snapshot en memoria cacheado y sincronizado.

## 2. El invariante

> **El adaptador de GitHub debe cumplir exactamente las mismas garantías y fallar ante los mismos escenarios de error que el almacén de memoria y Supabase, certificado por el mismísimo contrato ejecutable de Liskov.**

Y una segunda promesa de concurrencia nativa:

> **Ningún commit sobre un concepto puede prosperar si su base difiere de la cabeza actual en GitHub — garantizado mediante el CAS del parámetro `sha` de la API de contenidos o el avance atómico de la rama.**

## 3. La implementación mínima

El almacenamiento en GitHub mapea el dominio de memoria a conceptos nativos de Git:

- **Documentos** = archivos `.md` almacenados en un directorio de una rama. Los bytes del archivo dictan la verdad.
- **CAS** = el parámetro `sha` de la API de *contents*. Un `PUT` o `DELETE` falla si el archivo cambió debajo, replicando la garantía de la memoria.
- **Historia** = *trailers* estructurados en los mensajes de commit (ej. `Memory-Rev: 1|commit|concepto|...`). La historia se reconstruye procesando los mensajes de commit paginados, sin releer blobs antiguos.
- **Lote atómico** = Para garantizar que todos los cambios de una revisión se aplican juntos o no se aplica ninguno, se usa la API de Git data: se crean *blobs*, luego un *tree*, luego un *commit*, y finalmente se actualiza la *ref* de la rama sin usar *force*.

Debido a que operaciones como `memory_search` o `memory_backlinks` necesitan el grafo completo, el adaptador materializa un **snapshot** de lectura en memoria. Se cachea utilizando el `sha` de la cabeza (HEAD): si la rama no ha avanzado en GitHub, la lectura se responde desde la RAM sin volver a la red.

## 4. Una versión deliberadamente rota

Imagina intentar hacer un lote de cambios (múltiples actualizaciones) usando únicamente la API de archivos (contents API) de forma secuencial:

```rust
// ❌ NO HACER: Múltiples peticiones secuenciales sin atomicidad
self.put_file("a.md", content_a, "Update A").await?;
// Si esta petición falla, el estado queda inconsistente (lote parcial)
self.put_file("b.md", content_b, "Update B").await?;
```

La versión rota nace de pensar en GitHub como un disco remoto donde escribimos archivo por archivo.

## 5. Por qué falla

La API de contenidos de GitHub crea un commit *por cada archivo alterado*. Si el proceso se interrumpe o la segunda escritura sufre un conflicto de red (un agente paralelo actualizó `b.md`), nos quedamos con un "lote parcial" visible para todos. Esto rompe la propiedad de atomicidad. En contraste, la API de bajo nivel de Git Data (árboles y referencias) permite construir la revisión completa desconectada y solo mover el puntero de la rama al final. Si la rama avanzó mientras construíamos el commit, la actualización de la referencia falla y todo el lote se rechaza limpio: un verdadero todo-o-nada.

## 6. Memoria y asignación

En lugar de mantener únicamente una conexión de base de datos como hace `SupabaseStore`, `GithubStore` materializa todo el repositorio en RAM dentro de un snapshot. El coste de red hace prohibitivo pedir archivos sueltos. 

Para que este snapshot cacheado pueda ser compartido entre múltiples hilos de forma segura (`Send + Sync`), utilizamos concurrencia estándar de Rust: protegemos la caché con un `Mutex`. El snapshot entero vive en un `Mutex<Option<Snapshot>>` y se reemplaza atómicamente cuando el sha de HEAD indica que la rama se ha movido.

## 7. Tests

La prueba de fuego de que `GithubStore` es un sustituto exacto reside en su integración ([contract.rs](../crates/github-store/tests/contract.rs)). Ejecuta la misma suite `run_all` que ya pasa `InMemoryStore` y `SupabaseStore`, pero apuntando a un servidor Axum falso local (`fake_github`) que simula la API REST de GitHub (refs, trees, contents, CAS por blob sha y paginación de commits).

```rust
#[test]
fn github_store_cumple_el_contrato() {
    let server = fake_github::spawn();
    store_core::contract::run_all(|| {
        server.reset();
        GithubStore::new(server.base_url(), "owner", "repo", "main", "memoria", None)
    });
}
```

## 8. Frontera de producción

> 🧰 **La rueda de serie:** GitHub puede ser la fuente de verdad perfecta, pero carece de indexación compleja. Un cliente robusto usaría [`reqwest`](https://docs.rs/reqwest) para las llamadas REST (como hace este adaptador). El mapa completo y el criterio para elegir: [La rueda de serie](la-rueda-de-serie.md).

La conclusión ineludible al operar contra GitHub es que **la búsqueda necesita un índice derivado**. Sin una base de datos secundaria, `embed_pending` y las búsquedas semánticas no pueden ser eficientes o directamente devuelven vacío (igual que `InMemoryStore`). 

Para resolver este límite de rendimiento, implementamos el adaptador compuesto **`IndexedStore<G, S>`**, estructurando una arquitectura CQRS pura:
1.  **Escrituras (Commands)**: Van de forma sincrónica e inmediata directamente a GitHub (`G`), que actúa como la fuente de verdad absoluta y duradera.
2.  **Lecturas y Búsquedas (Queries)**: Se resuelven de forma rápida contra Supabase (`S`), que almacena los embeddings vectoriales (pgvector) y sirve de índice semántico optimizado.
3.  **Sincronización robusta**: Al guardar, se realiza una escritura inmediata de "mejor esfuerzo" en Supabase. Si esta falla o si hay cambios directos en Git, el reconciliador periódico del outbox alinea la base de datos con la realidad de GitHub.

De este modo, se tiene el control de cambios completo de Git y la velocidad semántica vectorial de Supabase en paralelo.

## 9. Principios SOLID en juego

* **L (Sustituibilidad de Liskov) en su máxima expresión:** `GithubStore` pasa el mismo contrato ejecutable que las otras dos implementaciones, traduciendo de forma transparente entre la memoria, PostgreSQL y los commits de git.
* **D (Inversión de Dependencias):** El entry point de la aplicación no se acopla a un adaptador concreto. En tiempo de despliegue, inyecta el `GithubStore` o `SupabaseStore` simplemente configurando una variable de entorno `OKF_STORE`.

## 10. Ejercicios

1. **Guiado.** El código usa `Mutex` para proteger la caché. Compara esta elección con un diseño hipotético que usara `RefCell`. ¿Por qué `RefCell` rompe la posibilidad de compartir el adaptador a través del servidor MCP en `tokio`? Justifica en términos de `Send` y `Sync`.
2. **Medio.** ¿Qué pasa si ocurre un conflicto de SHA en la API? Modifica (mentalmente o en el código) el flujo de `put_file` para implementar un retry automático vaciando la caché si devuelve un código 409 Conflict.
3. **Abierto.** Diseña un sketch de arquitectura donde GitHub almacene la verdad absoluta y Supabase sirva exclusivamente como un índice derivado para búsqueda semántica. ¿Quién lanza el webhook y quién aplica el cambio a pgvector?
