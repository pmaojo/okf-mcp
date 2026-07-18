# Capítulo 17 — Mantenimiento y Observabilidad: Diagnóstico e Integridad

Crates: [`crates/store-core`](../crates/store-core/src/lib.rs) · [`crates/supabase-store`](../crates/supabase-store/src/lib.rs) · [`crates/memory-tools`](../crates/memory-tools/src/lib.rs)

Un grafo de memoria que crece con commits de múltiples agentes y se sincroniza en segundo plano mediante un outbox asíncrono tiende inevitablemente a la entropía. Con el tiempo, aparecen enlaces rotos (documentos que apuntan a conceptos eliminados o inexistentes), embeddings ausentes u obsoletos (debido a fallos de red temporales con la API de Gemini) y eventos atascados en la cola de salida.

Si un cliente — o el propio sistema de control — tiene que leer y parsear todos los documentos uno a uno para detectar estos fallos de integridad, el sistema se vuelve lento e inmanejable. La observabilidad y el mantenimiento no deben ser un añadido de última hora; deben ser parte del diseño estructural del almacén.

## 1. El problema

¿Por qué no poner los métodos de diagnóstico en el mismo trait `MemoryRepository`? Al fin y al cabo, todo accede a la base de datos. 

Si añadimos `link_health`, `validate`, `stats`, `status` y `embed_pending` a `MemoryRepository`, cualquier consumidor que solo necesite leer y escribir documentos (como el transport stdio para responder a un commit sencillo) se verá obligado a importar y conocer toda la superficie de diagnóstico. Aún peor: si queremos implementar un mock sencillo o una base de datos en memoria para pruebas rápidas, nos veremos obligados a escribir decenas de líneas de código repetitivo para calcular métricas que no usaremos en el test.

Esto acopla el flujo operacional (escribir/leer la cabeza de un concepto) con el flujo de mantenimiento (reparar embeddings, computar hubs del grafo).

El invariante de diseño:

> **El ciclo de vida de los documentos y el diagnóstico de la salud del sistema son responsabilidades de clientes distintos. Un lector/escritor operacional jamás debe arrastrar la superficie de mantenimiento.**

## 2. El invariante

Para lograr esto de forma robusta en Rust:

1. **Segregación de Interfaces (SOLID-I)**: Dividimos el repositorio en dos traits independientes: `MemoryRepository` (operaciones de lectura/escritura CAS) y `StoreMaintenance` (observabilidad, validaciones y mantenimiento de embeddings).
2. **Presupuesto acotado (Budget)**: Las consultas agregadas o de diagnóstico deben respetar los límites de memoria y tamaño de búsqueda del cliente (`budget.max_search_results`), evitando desbordamientos de RAM y lecturas completas e ineficientes de tablas relacionales.

## 3. La implementación mínima

El trait `StoreMaintenance` se declara en `store-core` de forma totalmente independiente:

```rust
pub trait StoreMaintenance {
    /// Clasifica los enlaces salientes de un concepto según su salud.
    fn link_health(&self, id: &ConceptId) -> Result<LinkHealth, StoreError>;

    /// Valida el grafo completo (o un subárbol bajo un prefijo).
    fn validate(
        &self,
        path_prefix: Option<&str>,
        budget: &Budget,
    ) -> Result<ValidationReport, StoreError>;

    /// Métricas del grafo (hubs, huérfanos, recuentos por tipo y tag).
    fn stats(&self, budget: &Budget) -> Result<GraphStats, StoreError>;

    /// Estado operativo rápido en tiempo real.
    fn status(&self) -> Result<StoreStatus, StoreError>;

    /// Indexación semántica manual/bajo demanda de conceptos pendientes.
    fn embed_pending(
        &mut self,
        path_prefix: Option<&str>,
        max: usize,
    ) -> Result<EmbedOutcome, StoreError>;
}
```

Un cliente MCP que necesite herramientas de control expondrá 13 herramientas JSON-RPC (`memory_delete`, `memory_list`, `memory_backlinks`, `memory_embed`, etc.). Las herramientas operacionales dependerán únicamente de `MemoryRepository`, mientras que las de observabilidad se apoyarán en `StoreMaintenance`.

## 4. La versión rota (El anti-patrón de interfaz monolítica)

Imagina que decidimos meter todo en un único trait:

```rust
// ANTES: Interfaz monolítica que rompe SOLID-I
pub trait Repository {
    fn get(&self, id: &ConceptId) -> Result<Option<DocumentView>, StoreError>;
    fn commit(&mut self, req: CommitRequest, actor: &Principal, budget: &Budget) -> Result<CommitOutcome, StoreError>;
    
    // Métodos intrusivos de diagnóstico:
    fn stats(&self) -> Result<GraphStats, StoreError>;
    fn validate(&self) -> Result<ValidationReport, StoreError>;
}
```

Si queremos construir un adaptador de lectura en caché simple (un decorador en RAM que envuelve al repositorio real para acelerar lecturas), nos vemos obligados a implementar métodos complejos como `validate` u obligar al decorador a reenviar todas las llamadas, incrementando el acoplamiento y el código boilerplate de forma innecesaria.

## 5. Por qué falla

La violación de la **I** de SOLID (Interface Segregation Principle) introduce dos problemas graves:
* **Fragilidad**: Cualquier cambio en la estructura de diagnóstico (por ejemplo, añadir un nuevo campo a `GraphStats`) obliga a recompilar y modificar todos los adaptadores operacionales, aunque estos jamás lean estadísticas.
* **Falta de cohesión**: Mezcla el código de persistencia CAS transaccional rápido con complejas consultas de agregación y joins SQL en frío de base de datos.

## 6. Memoria y asignación

Al realizar análisis sobre el grafo (como buscar conceptos huérfanos o los nodos más enlazados en `stats`), un enfoque ingenuo traería toda la tabla `links` a la memoria RAM de la aplicación para calcular los grados de entrada.

En la versión en memoria (`MemoryStore`), mitigamos esto recorriendo estructuras ya en RAM de forma acotada. En la versión de producción (`SupabaseStore`), delegamos el cálculo directamente a la base de datos utilizando el poder del motor relacional PostgreSQL mediante consultas con límites explícitos (`LIMIT $1`). De esta manera, el consumo de memoria en el servidor Rust es constante \(O(1)\) respecto al tamaño del grafo.

## 7. Tests

Validamos que el diagnóstico sea correcto a través de los tests de integración en `mcp-stdio/tests/integration.rs`, verificando por ejemplo que un concepto que no es enlazado por ningún otro documento sea reportado correctamente como huérfano, y que al borrar un concepto lógicamente (`memory_delete`), sus enlaces de retroceso (backlinks) desaparezcan limpiamente de la vista general.

## 8. Frontera de producción

En un entorno real (Supabase/PostgreSQL), las columnas como `deleted_at TIMESTAMP WITH TIME ZONE` en la tabla `heads` y los índices dedicados sobre la tabla de enlaces (`CREATE INDEX idx_links_target ON links(target_id)`) son fundamentales. Sin este índice de backlinks, buscar los enlaces entrantes a un concepto forzaría a la base de datos a realizar un escaneo secuencial completo (*sequential scan*) sobre toda la tabla `links`, lo cual degradaría gravemente el rendimiento cuando el grafo alcance decenas de miles de relaciones.

## 9. Principios SOLID en juego

* **I (Interface Segregation)**: `StoreMaintenance` y `MemoryRepository` dividen las responsabilidades operacionales de las de observabilidad y reparación.
* **S (Single Responsibility)**: El worker asíncrono (`outbox-worker`) se encarga exclusivamente del procesado en segundo plano de eventos pendientes contra APIs de terceros (GitHub, Gemini), liberando al servidor principal de esa latencia sin comprometer la consistencia.

## 10. Ejercicios

1. **Reparación automática**: Implementa una herramienta `memory_repair` que busque todos los enlaces rotos (`validate`) y, de forma automática, cree stub-concepts vacíos con una etiqueta `type: placeholder` para restaurar la coherencia del grafo.
2. **Alertas de salud**: Añade un método al trait `StoreMaintenance` que devuelva un booleano `is_healthy` basado en si el número de enlaces rotos supera un umbral parametrizado en la configuración.
