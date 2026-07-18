//! Recorrido acotado del grafo de conocimiento.
//!
//! SOLID en juego:
//! - **D (inversión de dependencias):** este crate NO conoce el
//!   almacén. Define el trait [`NeighborSource`] y quien tenga los
//!   datos lo implementa. El algoritmo depende de una abstracción.
//! - **I (segregación de interfaces):** el trait pide UNA cosa
//!   (`neighbors`), no un repositorio entero. Un test lo implementa
//!   con un `BTreeMap` en tres líneas.
//! - **S:** aquí solo vive el algoritmo. Presupuestos en
//!   `memory-model`, datos en `memory-store`.
//!
//! El BFS es acotado por diseño: profundidad, número de nodos y
//! bytes acumulados. En serverless, "cargar el grafo entero" es un
//! bug de memoria esperando tráfico.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use memory_model::{Budget, ConceptId};
use std::collections::{BTreeSet, VecDeque};

/// Fuente de adyacencia. `Err` representa un fallo de E/S del
/// backend (en memoria nunca falla; sobre Postgres sí puede).
pub trait NeighborSource {
    /// Fallo de E/S del backend al leer adyacencia o tamaños.
    type Error;

    /// Vecinos salientes de `id`. Un id desconocido devuelve lista
    /// vacía, no error: los enlaces rotos son normales en una base
    /// de conocimiento viva.
    fn neighbors(&self, id: &ConceptId) -> Result<Vec<ConceptId>, Self::Error>;

    /// Tamaño en bytes del documento, para el presupuesto de bytes.
    /// `None` si el concepto no existe (enlace roto).
    fn document_size(&self, id: &ConceptId) -> Result<Option<usize>, Self::Error>;
}

/// Un nodo visitado y a qué distancia del origen se encontró.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Visited {
    /// El concepto visitado.
    pub id: ConceptId,
    /// Distancia en saltos desde el origen (el origen es 0).
    pub depth: u8,
    /// `false` si el enlace apunta a un concepto que no existe.
    pub exists: bool,
    /// El nodo que lo descubrió durante el BFS. `None` solo para el
    /// origen. Con esto el resultado deja de ser un conjunto plano de
    /// nodos y pasa a ser un árbol reconstruible — la arista real que
    /// necesita, por ejemplo, una visualización de grafo.
    pub parent: Option<ConceptId>,
}

/// Resultado del recorrido, con las razones de truncado explícitas.
/// El agente que consume esto DEBE poder saber si vio todo o no.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Traversal {
    /// En orden BFS (el origen primero).
    pub visited: Vec<Visited>,
    /// Se alcanzó `budget.max_graph_nodes` con nodos pendientes.
    pub truncated_by_nodes: bool,
    /// Se alcanzó `budget.max_graph_depth` con vecinos sin explorar.
    pub truncated_by_depth: bool,
    /// Los bytes acumulados superaron `budget.max_response_bytes`.
    pub truncated_by_bytes: bool,
    /// Bytes de documentos existentes acumulados.
    pub total_bytes: usize,
}

/// BFS acotado desde `start`.
///
/// Garantías (verificadas en tests):
/// - nunca visita más de `budget.max_graph_nodes` nodos;
/// - nunca desciende más de `budget.max_graph_depth` niveles;
/// - deja de ENCOLAR cuando los bytes acumulados superan
///   `budget.max_response_bytes` (el nodo que cruza el umbral se
///   incluye; sus vecinos ya no);
/// - termina en grafos con ciclos (conjunto `visited`).
///
/// # Ejemplo
///
/// La fuente puede ser cualquier cosa que sepa dar vecinos — aquí,
/// un `BTreeMap` de tres entradas (SOLID-I en acción):
///
/// ```
/// use graph_core::{bounded_bfs, NeighborSource};
/// use memory_model::{Budget, ConceptId};
/// use std::collections::BTreeMap;
/// use std::convert::Infallible;
///
/// struct MapGraph(BTreeMap<ConceptId, Vec<ConceptId>>);
///
/// impl NeighborSource for MapGraph {
///     type Error = Infallible;
///     fn neighbors(&self, id: &ConceptId) -> Result<Vec<ConceptId>, Infallible> {
///         Ok(self.0.get(id).cloned().unwrap_or_default())
///     }
///     fn document_size(&self, id: &ConceptId) -> Result<Option<usize>, Infallible> {
///         Ok(self.0.contains_key(id).then_some(100))
///     }
/// }
///
/// let id = |s: &str| ConceptId::parse(s).unwrap();
/// let g = MapGraph(BTreeMap::from([
///     (id("a"), vec![id("b"), id("c")]),
///     (id("b"), vec![id("a")]), // el ciclo no cuelga el recorrido
///     (id("c"), vec![]),
/// ]));
///
/// let t = bounded_bfs(&g, &id("a"), &Budget::default()).unwrap();
/// let orden: Vec<&str> = t.visited.iter().map(|v| v.id.as_str()).collect();
/// assert_eq!(orden, ["a", "b", "c"]);
/// assert!(!t.truncated_by_nodes);
/// ```
pub fn bounded_bfs<S: NeighborSource>(
    source: &S,
    start: &ConceptId,
    budget: &Budget,
) -> Result<Traversal, S::Error> {
    let mut visited_set: BTreeSet<ConceptId> = BTreeSet::new();
    let mut order: Vec<Visited> = Vec::new();
    let mut queue: VecDeque<(ConceptId, u8, Option<ConceptId>)> = VecDeque::new();
    let mut result = Traversal {
        visited: Vec::new(),
        truncated_by_nodes: false,
        truncated_by_depth: false,
        truncated_by_bytes: false,
        total_bytes: 0,
    };

    visited_set.insert(start.clone());
    queue.push_back((start.clone(), 0, None));

    while let Some((id, depth, parent)) = queue.pop_front() {
        if order.len() >= budget.max_graph_nodes {
            result.truncated_by_nodes = true;
            break;
        }

        let size = source.document_size(&id)?;
        let exists = size.is_some();
        if let Some(bytes) = size {
            // Aritmética comprobada: un overflow aquí sería un bug
            // silencioso de presupuesto.
            result.total_bytes = result.total_bytes.saturating_add(bytes);
        }
        order.push(Visited { id: id.clone(), depth, exists, parent });

        let over_bytes = result.total_bytes > budget.max_response_bytes;
        if over_bytes {
            result.truncated_by_bytes = true;
        }

        if depth >= budget.max_graph_depth {
            // Solo es truncado real si había vecinos que explorar.
            if exists && !source.neighbors(&id)?.is_empty() {
                result.truncated_by_depth = true;
            }
            continue;
        }
        if over_bytes || !exists {
            continue;
        }

        for neighbor in source.neighbors(&id)? {
            if visited_set.insert(neighbor.clone()) {
                queue.push_back((neighbor, depth + 1, Some(id.clone())));
            }
        }
    }

    if !queue.is_empty() && !result.truncated_by_nodes && !result.truncated_by_bytes {
        // Quedaron nodos encolados sin procesar: solo puede pasar por
        // el corte de nodos, así que este camino no debería darse;
        // lo dejamos como cinturón y tirantes.
        result.truncated_by_nodes = true;
    }

    result.visited = order;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::convert::Infallible;

    /// Implementación de juguete: SOLID-I en acción. Tres líneas de
    /// datos bastan para probar el algoritmo sin ningún almacén.
    struct MapGraph {
        edges: BTreeMap<ConceptId, Vec<ConceptId>>,
        sizes: BTreeMap<ConceptId, usize>,
    }

    impl NeighborSource for MapGraph {
        type Error = Infallible;
        fn neighbors(&self, id: &ConceptId) -> Result<Vec<ConceptId>, Infallible> {
            Ok(self.edges.get(id).cloned().unwrap_or_default())
        }
        fn document_size(&self, id: &ConceptId) -> Result<Option<usize>, Infallible> {
            Ok(self.sizes.get(id).copied())
        }
    }

    fn id(s: &str) -> ConceptId {
        ConceptId::parse(s).unwrap()
    }

    fn graph(edges: &[(&str, &[&str])]) -> MapGraph {
        let mut e = BTreeMap::new();
        let mut sizes = BTreeMap::new();
        for (from, tos) in edges {
            e.insert(id(from), tos.iter().map(|t| id(t)).collect());
            sizes.insert(id(from), 100);
            for t in *tos {
                sizes.entry(id(t)).or_insert(100);
            }
        }
        MapGraph { edges: e, sizes }
    }

    #[test]
    fn recorre_en_orden_bfs() {
        let g = graph(&[("a", &["b", "c"]), ("b", &["d"]), ("c", &["d"])]);
        let t = bounded_bfs(&g, &id("a"), &Budget::default()).unwrap();
        let ids: Vec<&str> = t.visited.iter().map(|v| v.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b", "c", "d"]);
        assert_eq!(t.visited[3].depth, 2);
        assert!(!t.truncated_by_nodes && !t.truncated_by_depth && !t.truncated_by_bytes);
    }

    #[test]
    fn termina_con_ciclos() {
        let g = graph(&[("a", &["b"]), ("b", &["a"])]);
        let t = bounded_bfs(&g, &id("a"), &Budget::default()).unwrap();
        assert_eq!(t.visited.len(), 2);
    }

    #[test]
    fn respeta_max_nodes() {
        let hijos: Vec<String> = (0..20).map(|i| format!("hijo-{i}")).collect();
        let refs: Vec<&str> = hijos.iter().map(|s| s.as_str()).collect();
        let g = graph(&[("raiz", refs.as_slice())]);
        let budget = Budget { max_graph_nodes: 5, ..Budget::default() };
        let t = bounded_bfs(&g, &id("raiz"), &budget).unwrap();
        assert_eq!(t.visited.len(), 5);
        assert!(t.truncated_by_nodes);
    }

    #[test]
    fn respeta_max_depth() {
        let g = graph(&[("a", &["b"]), ("b", &["c"]), ("c", &["d"]), ("d", &["e"])]);
        let budget = Budget { max_graph_depth: 2, ..Budget::default() };
        let t = bounded_bfs(&g, &id("a"), &budget).unwrap();
        let ids: Vec<&str> = t.visited.iter().map(|v| v.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b", "c"]);
        assert!(t.truncated_by_depth);
    }

    #[test]
    fn respeta_presupuesto_de_bytes() {
        let g = graph(&[("a", &["b"]), ("b", &["c"]), ("c", &["d"])]);
        let budget = Budget { max_response_bytes: 150, ..Budget::default() };
        let t = bounded_bfs(&g, &id("a"), &budget).unwrap();
        assert_eq!(t.visited.len(), 2);
        assert!(t.truncated_by_bytes);
    }

    #[test]
    fn enlaces_rotos_no_rompen_el_recorrido() {
        let mut g = graph(&[("a", &["fantasma"])]);
        g.sizes.remove(&id("fantasma"));
        let t = bounded_bfs(&g, &id("a"), &Budget::default()).unwrap();
        assert_eq!(t.visited.len(), 2);
        assert!(!t.visited[1].exists);
    }

    #[test]
    fn el_origen_no_tiene_padre_y_los_demas_reconstruyen_un_arbol_sin_ciclos() {
        let g = graph(&[("a", &["b", "c"]), ("b", &["d"]), ("c", &["d"])]);
        let t = bounded_bfs(&g, &id("a"), &Budget::default()).unwrap();

        let by_id: BTreeMap<&str, &Visited> =
            t.visited.iter().map(|v| (v.id.as_str(), v)).collect();
        assert_eq!(by_id["a"].parent, None);

        // Desde cualquier nodo visitado, subir por `parent` debe
        // terminar SIEMPRE en la raíz (`None`) en, como mucho,
        // `visited.len()` pasos — si hubiera un ciclo en la cadena de
        // padres, este bucle no terminaría nunca sin el corte.
        for v in &t.visited {
            let mut current = v.id.clone();
            let mut steps = 0;
            loop {
                let node = by_id[current.as_str()];
                match &node.parent {
                    None => break,
                    Some(p) => current = p.clone(),
                }
                steps += 1;
                assert!(steps <= t.visited.len(), "ciclo detectado en la cadena de padres");
            }
        }
    }
}
