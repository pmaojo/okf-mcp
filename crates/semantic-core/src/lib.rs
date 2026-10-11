//! Consolidación semántica determinista: de episodios estructurados a
//! hechos derivados, sin LLM y sin tocar nada que no sea la capa
//! derivada.
//!
//! Idea (arXiv 2607.01988, *Episodic-to-Semantic Consolidation Without
//! Identity Drift*): consolidar NO es mutar al agente ni sus
//! conceptos curados, sino calcular una función pura
//! `f: episodios -> hechos` cuya salida vive en un espacio de ids
//! aparte (`facts/…`, `runs/…`). Aquí esa separación es de TIPOS, no
//! una aserción en runtime: [`distill`] solo produce
//! [`DerivedWrite`], y un [`DerivedWrite`] solo se puede construir con
//! un [`DerivedId`], que por construcción vive bajo `facts/` o
//! `runs/`. Un llamador que quiera escribir en `people/alice` desde
//! aquí ni siquiera compila.
//!
//! Propiedades que este crate garantiza (y que sus tests verifican):
//!
//! - **Determinismo**: mismos episodios + mismos [`Params`] +
//!   [`RULE_VERSION`] ⇒ mismos bytes. Sin reloj, sin azar, sin E/S.
//! - **Independencia del orden**: los agregados son contadores y
//!   extremos (conmutativos y asociativos) sobre un conjunto ordenado
//!   por id; barajar la entrada no cambia la salida.
//! - **Idempotencia**: el hecho se RECALCULA desde los episodios
//!   vivos en vez de acumularse sobre el anterior, así repetir la
//!   pasada (o reanudarla tras una caída) no puede contar dos veces.
//! - **Auditoría**: cada hecho enlaza con `[[derived_from:…]]` a los
//!   episodios que lo sostienen y cada pasada deja un
//!   `type: consolidation-run` que enlaza con `[[produced:…]]` a los
//!   hechos. El hecho NO enlaza al run: si lo hiciera, su contenido
//!   cambiaría en cada pasada y la idempotencia se perdería.
//!
//! Fuera de alcance (v1): abstracción lingüística sobre prosa,
//! decaimiento temporal y defensa frente a episodios envenenados.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use memory_model::{Budget, ConceptId};
use okf_core::{FmValue, OkfDocument};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// Versión de las reglas de agregación y de la fórmula de confianza.
/// Va en cada hecho: cambiar la fórmula exige subirla, y los hechos
/// de la versión anterior dejan de ser byte-iguales a propósito.
pub const RULE_VERSION: &str = "v1";

/// Prefijo de los hechos derivados.
pub const FACTS_PREFIX: &str = "facts";
/// Prefijo de los registros de pasada.
pub const RUNS_PREFIX: &str = "runs";
/// Prefijo por defecto donde viven los episodios.
pub const EPISODES_PREFIX: &str = "episodes";

/// Longitud máxima de cada componente normalizado (skill, target,
/// contexto, motivo de fallo). Un valor más largo hace que el
/// episodio se SALTE en vez de truncarse: truncar fusionaría en
/// silencio grupos distintos.
pub const MAX_COMPONENT_LEN: usize = 48;

/// Episodios de soporte que cada hecho enlaza como máximo (los de
/// mayor id; `n_obs` conserva el total real).
pub const MAX_SUPPORTING_LINKS: usize = 16;

/// Hechos que un run enlaza como máximo.
pub const MAX_RUN_FACT_LINKS: usize = 100;

/// Valor por defecto cuando un episodio no declara `context`.
pub const DEFAULT_CONTEXT: &str = "default";

/// Normaliza un texto a un componente de [`ConceptId`]: minúsculas
/// ASCII, dígitos, `-` y `_`; todo lo demás colapsa a un único `-`.
/// Devuelve `None` si queda vacío o supera [`MAX_COMPONENT_LEN`].
///
/// ```
/// use semantic_core::slug;
/// assert_eq!(slug("Glass Cup!"), Some("glass-cup".to_string()));
/// assert_eq!(slug("   "), None);
/// ```
pub fn slug(raw: &str) -> Option<String> {
    let mut out = String::new();
    let mut pending_dash = false;
    for ch in raw.trim().chars() {
        let c = ch.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(c);
        } else {
            pending_dash = true;
        }
    }
    if out.is_empty() || out.len() > MAX_COMPONENT_LEN {
        None
    } else {
        Some(out)
    }
}

/// Resultado de un episodio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// El intento salió bien.
    Success,
    /// El intento falló.
    Failure,
}

/// Un episodio estructurado: lo único que la v1 sabe agregar.
///
/// Los campos ya están normalizados con [`slug`], de modo que la
/// clave de grupo es comparable byte a byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Episode {
    /// Concepto del que salió.
    pub id: ConceptId,
    /// Capacidad ejercida (p. ej. `manipulation-grasp`).
    pub skill: String,
    /// Objeto o destino (p. ej. `glass-cup`).
    pub target: String,
    /// Entorno (p. ej. `sim-relaxed`); [`DEFAULT_CONTEXT`] si falta.
    pub context: String,
    /// Resultado.
    pub outcome: Outcome,
    /// Motivo del fallo, si lo hubo y se declaró.
    pub failure_reason: Option<String>,
}

fn scalar<'a>(doc: &'a OkfDocument, key: &str) -> Option<&'a str> {
    match doc.extra.get(key) {
        Some(FmValue::Scalar(s)) => Some(s.as_str()),
        _ => None,
    }
}

/// Extrae un [`Episode`] de un documento OKF, o `None` si no es un
/// episodio estructurado completo y bien formado.
///
/// Se exige `type: episode`, `skill`, `target` y
/// `outcome: success|failure`. `context` es opcional. Un episodio al
/// que le falte estructura se IGNORA: la v1 no intenta extraer nada
/// de prosa libre.
pub fn extract(id: &ConceptId, doc: &OkfDocument) -> Option<Episode> {
    if doc.doc_type != "episode" {
        return None;
    }
    let skill = slug(scalar(doc, "skill")?)?;
    let target = slug(scalar(doc, "target")?)?;
    let context = match scalar(doc, "context") {
        Some(raw) => slug(raw)?,
        None => DEFAULT_CONTEXT.to_string(),
    };
    let outcome = match scalar(doc, "outcome")?.trim() {
        "success" => Outcome::Success,
        "failure" => Outcome::Failure,
        _ => return None,
    };
    let failure_reason = match (outcome, scalar(doc, "failure_reason")) {
        (Outcome::Failure, Some(raw)) => Some(slug(raw)?),
        _ => None,
    };
    Some(Episode { id: id.clone(), skill, target, context, outcome, failure_reason })
}

/// Parámetros de la fórmula de confianza. Forman parte de la regla:
/// se escriben en cada hecho, así que cambiarlos cambia los bytes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    /// Tasa a la que se encoge una tasa con pocas observaciones.
    pub prior_mean: f64,
    /// Peso del prior, en observaciones equivalentes.
    pub prior_weight: f64,
}

impl Default for Params {
    /// Los valores que el paper fija ex ante (0,5 y 10).
    fn default() -> Self {
        Params { prior_mean: 0.5, prior_weight: 10.0 }
    }
}

/// Tasa encogida hacia el prior:
/// `(éxitos + prior_mean · w) / (n + w)`.
///
/// Con pocos datos manda el prior; con muchos, la tasa observada.
///
/// ```
/// use semantic_core::{shrunk_rate, Params};
/// let p = Params::default();
/// assert_eq!(shrunk_rate(0, 0, &p), 0.5);
/// assert!((shrunk_rate(12, 15, &p) - 0.68).abs() < 1e-12);
/// ```
pub fn shrunk_rate(successes: u64, n: u64, p: &Params) -> f64 {
    (successes as f64 + p.prior_mean * p.prior_weight) / (n as f64 + p.prior_weight)
}

/// Confianza en `[0, 1]`: `n / (n + w) · (2q − 1)²`, con `q` la tasa
/// encogida.
///
/// El primer factor crece con las observaciones (2 datos pesan
/// mucho menos que 200); el segundo es 0 cuando la tasa está cerca
/// de 0,5 —un resultado que no distingue el éxito del fallo— y 1 en
/// los extremos. La fórmula exacta importa menos que hacer visible
/// la incertidumbre, pero está versionada por [`RULE_VERSION`].
///
/// ```
/// use semantic_core::{confidence, Params};
/// let p = Params::default();
/// assert_eq!(confidence(0, 0, &p), 0.0);
/// assert!(confidence(200, 200, &p) > confidence(2, 2, &p));
/// ```
pub fn confidence(successes: u64, n: u64, p: &Params) -> f64 {
    let q = shrunk_rate(successes, n, p);
    let evidence = n as f64 / (n as f64 + p.prior_weight);
    let separation = (2.0 * q - 1.0) * (2.0 * q - 1.0);
    evidence * separation
}

/// Clase de hecho derivado. Vocabulario CERRADO: un agente no puede
/// introducir categorías nuevas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FactKind {
    /// Tasa de éxito por `skill/target/context`.
    SkillSuccessRate,
    /// Patrón de fallo por `skill/target/failure_reason`.
    InteractionPattern,
}

impl FactKind {
    /// Nombre estable del tipo, tal como aparece en el frontmatter.
    pub fn as_str(self) -> &'static str {
        match self {
            FactKind::SkillSuccessRate => "skill_success_rate",
            FactKind::InteractionPattern => "interaction_pattern",
        }
    }

    /// Segmento de ruta (los ids no admiten mayúsculas ni puntos,
    /// pero sí `_`; se conserva tal cual).
    fn path_segment(self) -> &'static str {
        self.as_str()
    }
}

/// Un hecho derivado ya agregado, listo para renderizar.
#[derive(Debug, Clone, PartialEq)]
pub struct Fact {
    /// Clase del hecho.
    pub kind: FactKind,
    /// Clave estable dentro de la clase (`skill/target/contexto`).
    pub key: String,
    /// Observaciones que lo sostienen.
    pub n_obs: u64,
    /// Éxitos entre ellas (en `interaction_pattern`, igual a `n_obs`
    /// de fallos: ver `failures`).
    pub successes: u64,
    /// Fallos entre ellas.
    pub failures: u64,
    /// Tasa observada de éxito (`successes / n_obs`).
    pub rate: f64,
    /// Tasa encogida hacia el prior.
    pub rate_shrunk: f64,
    /// Confianza, ver [`confidence`].
    pub confidence: f64,
    /// Motivo de fallo más frecuente (empate: el menor
    /// alfabéticamente), si hubo fallos con motivo.
    pub top_failure_reason: Option<String>,
    /// Mayor id de episodio que lo sostiene.
    pub last_supporting_event: ConceptId,
    /// Los episodios que lo sostienen (los [`MAX_SUPPORTING_LINKS`]
    /// de mayor id), en orden creciente.
    pub supporting: Vec<ConceptId>,
}

impl Fact {
    /// Id del concepto donde vive este hecho.
    pub fn concept_id(&self) -> DerivedId {
        let id = format!("{}/{}/{}", FACTS_PREFIX, self.kind.path_segment(), self.key);
        // Los componentes ya están normalizados con `slug`; solo
        // puede fallar por longitud total, y entonces el hecho no se
        // emite (ver `distill`).
        DerivedId::parse(&id).expect("componentes normalizados forman un id válido")
    }
}

/// Id de un concepto de la capa derivada: SIEMPRE bajo `facts/` o
/// `runs/`. Es el pasaporte de escritura de este crate.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DerivedId(ConceptId);

impl DerivedId {
    /// Acepta solo ids bajo `facts/…` o `runs/…`.
    ///
    /// ```
    /// use semantic_core::DerivedId;
    /// assert!(DerivedId::parse("facts/skill_success_rate/a/b/c").is_ok());
    /// assert!(DerivedId::parse("runs/distill-00").is_ok());
    /// assert!(DerivedId::parse("people/alice").is_err());
    /// assert!(DerivedId::parse("facts").is_err()); // sin hoja
    /// ```
    pub fn parse(raw: &str) -> Result<Self, SemanticError> {
        let id = ConceptId::parse(raw).map_err(|e| SemanticError::InvalidId(e.to_string()))?;
        match raw.split_once('/') {
            Some((FACTS_PREFIX, rest)) | Some((RUNS_PREFIX, rest)) if !rest.is_empty() => Ok(DerivedId(id)),
            _ => Err(SemanticError::OutsideDerivedLayer(raw.to_string())),
        }
    }

    /// El [`ConceptId`] subyacente.
    pub fn as_concept_id(&self) -> &ConceptId {
        &self.0
    }

    /// El id como texto.
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

/// Una escritura permitida en la capa derivada.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedWrite {
    /// Destino, garantizado bajo `facts/` o `runs/`.
    pub id: DerivedId,
    /// Documento OKF ya validado.
    pub markdown: String,
}

/// Por qué falló la destilación.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticError {
    /// Un id no cumple la lista blanca de [`ConceptId`].
    InvalidId(String),
    /// Un id cae fuera de `facts/` y `runs/`.
    OutsideDerivedLayer(String),
    /// El documento renderizado no pasó [`okf_core::parse_document`].
    Okf(okf_core::OkfError),
}

impl fmt::Display for SemanticError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SemanticError::InvalidId(e) => write!(f, "id inválido: {e}"),
            SemanticError::OutsideDerivedLayer(id) => {
                write!(f, "'{id}' cae fuera de la capa derivada (facts/ y runs/)")
            }
            SemanticError::Okf(e) => write!(f, "documento OKF inválido: {e}"),
        }
    }
}

impl std::error::Error for SemanticError {}

#[derive(Default)]
struct Group {
    successes: u64,
    failures: u64,
    reasons: BTreeMap<String, u64>,
    events: BTreeSet<ConceptId>,
}

/// Agrega episodios en hechos. Pura: sin E/S, reloj ni azar.
///
/// La entrada se trata como CONJUNTO (ids duplicados cuentan una
/// vez) y la salida es un `BTreeMap`: el orden en que lleguen los
/// episodios no puede influir en el resultado.
pub fn aggregate(episodes: &[Episode], params: &Params) -> BTreeMap<(FactKind, String), Fact> {
    let mut unique: BTreeMap<&ConceptId, &Episode> = BTreeMap::new();
    for e in episodes {
        unique.insert(&e.id, e);
    }

    let mut rates: BTreeMap<String, Group> = BTreeMap::new();
    let mut patterns: BTreeMap<String, Group> = BTreeMap::new();
    for e in unique.values() {
        let g = rates.entry(format!("{}/{}/{}", e.skill, e.target, e.context)).or_default();
        g.events.insert(e.id.clone());
        match e.outcome {
            Outcome::Success => g.successes += 1,
            Outcome::Failure => g.failures += 1,
        }
        if e.outcome == Outcome::Failure {
            if let Some(reason) = &e.failure_reason {
                let p = patterns.entry(format!("{}/{}/{}", e.skill, e.target, reason)).or_default();
                p.events.insert(e.id.clone());
                p.failures += 1;
                *g.reasons.entry(reason.clone()).or_default() += 1;
            }
        }
    }

    let mut out = BTreeMap::new();
    for (key, g) in rates {
        let n = g.successes + g.failures;
        out.insert((FactKind::SkillSuccessRate, key.clone()), build_fact(FactKind::SkillSuccessRate, key, &g, n, params));
    }
    for (key, g) in patterns {
        let n = g.failures;
        out.insert((FactKind::InteractionPattern, key.clone()), build_fact(FactKind::InteractionPattern, key, &g, n, params));
    }
    out
}

fn build_fact(kind: FactKind, key: String, g: &Group, n: u64, params: &Params) -> Fact {
    let (rate, rate_shrunk, conf) = match kind {
        FactKind::SkillSuccessRate => (
            g.successes as f64 / n as f64,
            shrunk_rate(g.successes, n, params),
            confidence(g.successes, n, params),
        ),
        // Un patrón de fallo solo cuenta fallos: no tiene tasa de
        // éxito. La confianza es puramente el peso de la evidencia.
        FactKind::InteractionPattern => (0.0, 0.0, n as f64 / (n as f64 + params.prior_weight)),
    };
    // Máximo por frecuencia; el empate lo gana el menor alfabéticamente.
    let top_failure_reason = g
        .reasons
        .iter()
        .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
        .map(|(r, _)| r.clone());
    let supporting: Vec<ConceptId> = g
        .events
        .iter()
        .rev()
        .take(MAX_SUPPORTING_LINKS)
        .rev()
        .cloned()
        .collect();
    let last_supporting_event = g
        .events
        .iter()
        .next_back()
        .expect("un grupo nunca está vacío")
        .clone();
    Fact {
        kind,
        key,
        n_obs: n,
        successes: g.successes,
        failures: g.failures,
        rate,
        rate_shrunk,
        confidence: conf,
        top_failure_reason,
        last_supporting_event,
        supporting,
    }
}

/// Formato fijo de coma flotante: 6 decimales. Las operaciones de
/// [`confidence`] son solo `+ - * /` de IEEE-754, que son exactas
/// entre plataformas, así que el texto es reproducible.
fn fnum(x: f64) -> String {
    format!("{x:.6}")
}

/// Renderiza un hecho como documento OKF y lo valida.
pub fn render_fact(fact: &Fact, params: &Params, budget: &Budget) -> Result<String, SemanticError> {
    let mut out = String::new();
    out.push_str("---\ntype: semantic-fact\n");
    out.push_str(&format!("title: {} {}\n", fact.kind.as_str().replace('_', " "), fact.key));
    out.push_str(&format!("tags:\n  - fact\n  - rule-{RULE_VERSION}\n"));
    out.push_str(&format!("fact_kind: {}\n", fact.kind.as_str()));
    out.push_str(&format!("fact_key: {}\n", fact.key));
    out.push_str(&format!("n_obs: {}\n", fact.n_obs));
    match fact.kind {
        FactKind::SkillSuccessRate => {
            out.push_str(&format!("successes: {}\n", fact.successes));
            out.push_str(&format!("failures: {}\n", fact.failures));
            out.push_str(&format!("rate: {}\n", fnum(fact.rate)));
            out.push_str(&format!("rate_shrunk: {}\n", fnum(fact.rate_shrunk)));
        }
        FactKind::InteractionPattern => {}
    }
    out.push_str(&format!("confidence: {}\n", fnum(fact.confidence)));
    if let Some(reason) = &fact.top_failure_reason {
        out.push_str(&format!("top_failure_reason: {reason}\n"));
    }
    out.push_str(&format!("last_supporting_event: {}\n", fact.last_supporting_event.as_str()));
    out.push_str(&format!("rule_version: {RULE_VERSION}\n"));
    out.push_str(&format!("prior_mean: {}\n", fnum(params.prior_mean)));
    out.push_str(&format!("prior_weight: {}\n", fnum(params.prior_weight)));
    out.push_str("---\n");
    out.push_str(&format!(
        "Hecho derivado de {} observaciones (regla {RULE_VERSION}). No editar a mano: se recalcula desde los episodios.\n",
        fact.n_obs
    ));
    out.push_str("\n## Soporte\n");
    for id in &fact.supporting {
        out.push_str(&format!("- [[derived_from:{}]]\n", id.as_str()));
    }
    okf_core::parse_document(&out, budget).map_err(SemanticError::Okf)?;
    Ok(out)
}

/// Plan completo de una pasada: qué escribir, nada más.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DistillPlan {
    /// Hechos (ordenados por id) y, al final, el run.
    pub writes: Vec<DerivedWrite>,
    /// Cuántos hechos hay.
    pub facts: usize,
    /// Cuántos episodios distintos se agregaron.
    pub episodes_used: usize,
}

/// Calcula la pasada completa: agrega, renderiza cada hecho y añade
/// el run de auditoría.
///
/// `episodes_skipped` (episodios sin estructura suficiente) solo se
/// registra en el run. El id del run es el SHA-256 truncado de todo
/// lo que contiene la pasada, así que una pasada idéntica a la
/// anterior produce el MISMO run y no escribe nada nuevo.
pub fn distill(
    episodes: &[Episode],
    episodes_skipped: usize,
    params: &Params,
    budget: &Budget,
) -> Result<DistillPlan, SemanticError> {
    let facts = aggregate(episodes, params);
    let mut writes = Vec::with_capacity(facts.len() + 1);
    let mut seed: Vec<u8> = Vec::new();
    seed.extend_from_slice(RULE_VERSION.as_bytes());
    seed.push(0);
    seed.extend_from_slice(episodes_skipped.to_string().as_bytes());
    seed.push(0);
    for fact in facts.values() {
        let id = fact.concept_id();
        let markdown = render_fact(fact, params, budget)?;
        seed.extend_from_slice(id.as_str().as_bytes());
        seed.push(0);
        seed.extend_from_slice(hash_core::sha256(markdown.as_bytes()).as_slice());
        writes.push(DerivedWrite { id, markdown });
    }
    writes.sort_by(|a, b| a.id.cmp(&b.id));

    let digest = hash_core::sha256(&seed);
    let short: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    let run_id = DerivedId::parse(&format!("{RUNS_PREFIX}/distill-{short}"))?;
    let episodes_used = {
        let mut ids: BTreeSet<&ConceptId> = BTreeSet::new();
        for e in episodes {
            ids.insert(&e.id);
        }
        ids.len()
    };
    let run = render_run(&writes, episodes_used, episodes_skipped, params, budget)?;
    let facts_len = writes.len();
    writes.push(DerivedWrite { id: run_id, markdown: run });
    Ok(DistillPlan { writes, facts: facts_len, episodes_used })
}

fn render_run(
    fact_writes: &[DerivedWrite],
    episodes_used: usize,
    episodes_skipped: usize,
    params: &Params,
    budget: &Budget,
) -> Result<String, SemanticError> {
    let mut out = String::new();
    out.push_str("---\ntype: consolidation-run\n");
    out.push_str("title: destilación determinista de hechos\n");
    out.push_str(&format!("tags:\n  - consolidation-run\n  - rule-{RULE_VERSION}\n"));
    out.push_str(&format!("rule_version: {RULE_VERSION}\n"));
    out.push_str(&format!("episodes_used: {episodes_used}\n"));
    out.push_str(&format!("episodes_skipped: {episodes_skipped}\n"));
    out.push_str(&format!("facts: {}\n", fact_writes.len()));
    out.push_str(&format!("prior_mean: {}\n", fnum(params.prior_mean)));
    out.push_str(&format!("prior_weight: {}\n", fnum(params.prior_weight)));
    out.push_str("---\n");
    out.push_str("Pasada de consolidación: agrega episodios estructurados en hechos; no modifica ningún concepto fuera de facts/ y runs/.\n");
    out.push_str("\n## Hechos producidos\n");
    for w in fact_writes.iter().take(MAX_RUN_FACT_LINKS) {
        out.push_str(&format!("- [[produced:{}]]\n", w.id.as_str()));
    }
    if fact_writes.len() > MAX_RUN_FACT_LINKS {
        out.push_str(&format!("\n(enlazados los primeros {MAX_RUN_FACT_LINKS} de {})\n", fact_writes.len()));
    }
    okf_core::parse_document(&out, budget).map_err(SemanticError::Okf)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cid(s: &str) -> ConceptId {
        ConceptId::parse(s).unwrap()
    }

    fn ep(n: usize, skill: &str, target: &str, ok: bool) -> Episode {
        Episode {
            id: cid(&format!("episodes/e{n:06}")),
            skill: skill.to_string(),
            target: target.to_string(),
            context: "default".to_string(),
            outcome: if ok { Outcome::Success } else { Outcome::Failure },
            failure_reason: if ok { None } else { Some("slip".to_string()) },
        }
    }

    /// Generador congruencial propio: el núcleo no tiene `rand`.
    fn shuffle<T>(v: &mut [T], mut state: u64) {
        for i in (1..v.len()).rev() {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let j = (state >> 33) as usize % (i + 1);
            v.swap(i, j);
        }
    }

    fn doc(raw: &str) -> OkfDocument {
        okf_core::parse_document(raw, &Budget::default()).unwrap()
    }

    #[test]
    fn extract_requires_structure() {
        let id = cid("episodes/a");
        let ok = doc("---\ntype: episode\ntitle: t\nskill: Manipulation Grasp\ntarget: glass_cup\noutcome: success\n---\ncuerpo\n");
        let e = extract(&id, &ok).unwrap();
        assert_eq!(e.skill, "manipulation-grasp");
        assert_eq!(e.context, DEFAULT_CONTEXT);
        assert_eq!(e.outcome, Outcome::Success);

        let no_outcome = doc("---\ntype: episode\ntitle: t\nskill: a\ntarget: b\n---\nx\n");
        assert!(extract(&id, &no_outcome).is_none());
        let bad_outcome = doc("---\ntype: episode\ntitle: t\nskill: a\ntarget: b\noutcome: maybe\n---\nx\n");
        assert!(extract(&id, &bad_outcome).is_none());
        let not_episode = doc("---\ntype: note\ntitle: t\nskill: a\ntarget: b\noutcome: success\n---\nx\n");
        assert!(extract(&id, &not_episode).is_none());
        let too_long = format!("---\ntype: episode\ntitle: t\nskill: {}\ntarget: b\noutcome: success\n---\nx\n", "a".repeat(60));
        assert!(extract(&id, &doc(&too_long)).is_none());
    }

    /// V1 del paper: 1000 episodios (800 éxitos, 200 fallos) dan un
    /// único hecho `skill_success_rate` con n=1000, 800 éxitos.
    #[test]
    fn v1_thousand_episodes_one_fact() {
        let eps: Vec<Episode> = (0..1000).map(|i| ep(i, "grasp", "cup", i % 5 != 0)).collect();
        let facts = aggregate(&eps, &Params::default());
        let f = &facts[&(FactKind::SkillSuccessRate, "grasp/cup/default".to_string())];
        assert_eq!((f.n_obs, f.successes, f.failures), (1000, 800, 200));
        assert!((f.rate - 0.8).abs() < 1e-12);
        assert_eq!(f.supporting.len(), MAX_SUPPORTING_LINKS);
        assert_eq!(f.last_supporting_event.as_str(), "episodes/e000999");
        let pattern = &facts[&(FactKind::InteractionPattern, "grasp/cup/slip".to_string())];
        assert_eq!(pattern.n_obs, 200);
    }

    #[test]
    fn order_of_input_does_not_matter() {
        let eps: Vec<Episode> = (0..300).map(|i| ep(i, "grasp", if i % 3 == 0 { "cup" } else { "ball" }, i % 4 != 0)).collect();
        let base = distill(&eps, 2, &Params::default(), &Budget::default()).unwrap();
        for seed in 1..=8u64 {
            let mut shuffled = eps.clone();
            shuffle(&mut shuffled, seed);
            let again = distill(&shuffled, 2, &Params::default(), &Budget::default()).unwrap();
            assert_eq!(base, again, "semilla {seed}");
        }
    }

    #[test]
    fn duplicates_count_once() {
        let mut eps: Vec<Episode> = (0..10).map(|i| ep(i, "g", "c", true)).collect();
        eps.extend(eps.clone());
        let facts = aggregate(&eps, &Params::default());
        assert_eq!(facts[&(FactKind::SkillSuccessRate, "g/c/default".to_string())].n_obs, 10);
    }

    #[test]
    fn same_input_same_bytes_and_same_run_id() {
        let eps: Vec<Episode> = (0..50).map(|i| ep(i, "g", "c", i % 2 == 0)).collect();
        let a = distill(&eps, 0, &Params::default(), &Budget::default()).unwrap();
        let b = distill(&eps, 0, &Params::default(), &Budget::default()).unwrap();
        assert_eq!(a, b);
        let more: Vec<Episode> = (0..51).map(|i| ep(i, "g", "c", i % 2 == 0)).collect();
        let c = distill(&more, 0, &Params::default(), &Budget::default()).unwrap();
        assert_ne!(a.writes.last().unwrap().id, c.writes.last().unwrap().id);
    }

    /// Un hecho nunca enlaza al run que lo produjo: si lo hiciera,
    /// su contenido cambiaría en cada pasada.
    #[test]
    fn fact_does_not_link_to_run_but_run_links_to_facts() {
        let eps: Vec<Episode> = (0..5).map(|i| ep(i, "g", "c", true)).collect();
        let plan = distill(&eps, 0, &Params::default(), &Budget::default()).unwrap();
        let budget = Budget::default();
        for w in &plan.writes[..plan.facts] {
            let d = okf_core::parse_document(&w.markdown, &budget).unwrap();
            assert!(d.links.iter().all(|l| l.target.as_str().starts_with("episodes/")));
            assert!(d.links.iter().all(|l| l.rel.as_deref() == Some("derived_from")));
        }
        let run = okf_core::parse_document(&plan.writes.last().unwrap().markdown, &budget).unwrap();
        assert_eq!(run.doc_type, "consolidation-run");
        assert!(run.links.iter().any(|l| l.rel.as_deref() == Some("produced")));
    }

    /// R6: todo lo que la pasada puede escribir cae bajo facts/ o runs/.
    #[test]
    fn writes_only_under_derived_layer() {
        let eps: Vec<Episode> = (0..30).map(|i| ep(i, "g", "c", i % 3 != 0)).collect();
        let plan = distill(&eps, 0, &Params::default(), &Budget::default()).unwrap();
        assert!(!plan.writes.is_empty());
        for w in &plan.writes {
            assert!(w.id.as_str().starts_with("facts/") || w.id.as_str().starts_with("runs/"));
        }
        for outside in ["people/alice", "specs/x", "skills/y", "facts", "runs", "factsx/a"] {
            assert!(DerivedId::parse(outside).is_err(), "{outside}");
        }
    }

    #[test]
    fn shrinkage_matches_paper_defaults() {
        let p = Params::default();
        // Con 12 de 15 la tasa encogida queda por encima de 0,5 y con
        // 3 de 15 por debajo (misma frontera del umbral del paper).
        assert!(shrunk_rate(12, 15, &p) > 0.5);
        assert!(shrunk_rate(3, 15, &p) < 0.5);
        // Pocas observaciones: manda el prior.
        assert!((shrunk_rate(2, 2, &p) - (2.0 + 5.0) / 12.0).abs() < 1e-12);
    }

    #[test]
    fn confidence_grows_with_evidence_and_dies_near_coin_flip() {
        let p = Params::default();
        assert!(confidence(200, 200, &p) > confidence(2, 2, &p));
        assert!(confidence(50, 100, &p) < 1e-9, "tasa 0,5 ⇒ confianza ~0");
        for n in 0..300u64 {
            for s in 0..=n {
                let c = confidence(s, n, &p);
                assert!((0.0..=1.0).contains(&c));
            }
        }
    }

    #[test]
    fn top_failure_reason_breaks_ties_alphabetically() {
        let mut eps = Vec::new();
        for (i, reason) in ["slip", "slip", "crush", "crush"].iter().enumerate() {
            let mut e = ep(i, "g", "c", false);
            e.failure_reason = Some(reason.to_string());
            eps.push(e);
        }
        let facts = aggregate(&eps, &Params::default());
        let f = &facts[&(FactKind::SkillSuccessRate, "g/c/default".to_string())];
        assert_eq!(f.top_failure_reason.as_deref(), Some("crush"));
    }

    #[test]
    fn rendered_fact_is_valid_okf_with_typed_links() {
        let eps = vec![ep(1, "g", "c", true), ep(2, "g", "c", false)];
        let plan = distill(&eps, 0, &Params::default(), &Budget::default()).unwrap();
        let fact = plan
            .writes
            .iter()
            .find(|w| w.id.as_str().starts_with("facts/skill_success_rate/"))
            .unwrap();
        let d = okf_core::parse_document(&fact.markdown, &Budget::default()).unwrap();
        assert_eq!(d.doc_type, "semantic-fact");
        assert!(matches!(d.extra.get("n_obs"), Some(FmValue::Scalar(s)) if s == "2"));
        assert!(fact.markdown.contains("rate_shrunk: "));
    }

    #[test]
    fn empty_input_yields_only_a_run() {
        let plan = distill(&[], 0, &Params::default(), &Budget::default()).unwrap();
        assert_eq!(plan.facts, 0);
        assert_eq!(plan.writes.len(), 1);
        assert!(plan.writes[0].id.as_str().starts_with("runs/"));
    }
}
