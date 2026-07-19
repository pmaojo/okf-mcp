//! Embeddings para pgvector, con respaldo multi-proveedor.
//!
//! Genera el vector de un texto probando varios proveedores en orden —
//! Gemini primero (histórico), con Mistral y Cohere como respaldo
//! automático si el primero falla (p. ej. cuota agotada, HTTP 429) o
//! no está configurado — y devuelve SIEMPRE junto al vector el
//! identificador exacto del modelo que lo produjo ([`Embedded`]).
//!
//! Por qué el identificador viaja pegado al vector: los embeddings de
//! proveedores distintos NO son comparables entre sí aunque compartan
//! dimensionalidad — cada modelo aprende su propio espacio vectorial,
//! y una distancia de coseno entre dos espacios distintos no produce
//! un error, produce un número sin ningún significado. A diferencia de
//! la síntesis de texto (cualquier proveedor devuelve markdown
//! igualmente válido), un fallback ciego aquí corrompería el ranking
//! semántico en silencio — peor que el fallo actual, que ya degrada
//! honestamente a coincidencia de texto.
//!
//! `okf-mcp` persiste `embedding_model` junto a cada vector (ver
//! `supabase-store::schema.sql`, columna `embeddings.embedding_model`)
//! y la búsqueda semántica SOLO compara vectores con el mismo
//! `model_id`: un documento indexado con el proveedor de respaldo
//! simplemente queda fuera del ranking semántico de una consulta
//! embebida con otro proveedor (sigue siendo encontrable por texto)
//! hasta que se re-indexe — degradación segura, nunca mezcla.
//!
//! `gemini-embedding-001` (truncado a 768 dims, embedding "Matryoshka":
//! el mismo vector es válido truncado a varias dimensionalidades) es
//! el proveedor primario. Los de respaldo son `mistral-embed`
//! (Mistral, 1024 dims fijas) y `embed-english-v3.0` (Cohere, 1024
//! dims fijas). Cohere, a diferencia de Gemini y Mistral, distingue
//! "documento" de "consulta" (`input_type`) — por eso hay dos
//! funciones de entrada, [`embed_document`] y [`embed_query`], en vez
//! de una sola.
//!
//! Un solo lugar para el nombre de modelo de cada proveedor:
//! `outbox-worker` (camino de escritura, genera el embedding de un
//! concepto recién commiteado) y `supabase-store` (camino de lectura,
//! genera el embedding de la consulta de búsqueda) DEBEN pasar por
//! aquí para que nunca puedan divergir en qué proveedor llamaron ni en
//! qué `model_id` etiquetaron.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

/// Modelo de Gemini y dimensionalidad truncada (`schema.sql` define la
/// columna como pensada originalmente para 768 dims).
pub const GEMINI_MODEL: &str = "gemini-embedding-001";
/// Dimensionalidad a la que se trunca el embedding "Matryoshka" de Gemini.
pub const GEMINI_DIMENSIONS: usize = 768;
/// Identificador persistido en `embeddings.embedding_model` para un
/// vector generado por Gemini.
pub const GEMINI_MODEL_ID: &str = "gemini:gemini-embedding-001@768";

/// Modelo de embeddings de Mistral (dimensionalidad fija, sin truncado).
pub const MISTRAL_MODEL: &str = "mistral-embed";
/// Dimensionalidad fija del modelo de Mistral.
pub const MISTRAL_DIMENSIONS: usize = 1024;
/// Identificador persistido en `embeddings.embedding_model` para un
/// vector generado por Mistral.
pub const MISTRAL_MODEL_ID: &str = "mistral:mistral-embed@1024";

/// Modelo de embeddings de Cohere (dimensionalidad fija, sin truncado).
pub const COHERE_MODEL: &str = "embed-english-v3.0";
/// Dimensionalidad fija del modelo de Cohere.
pub const COHERE_DIMENSIONS: usize = 1024;
/// Identificador persistido en `embeddings.embedding_model` para un
/// vector generado por Cohere.
pub const COHERE_MODEL_ID: &str = "cohere:embed-english-v3.0@1024";

/// Claves de proveedor disponibles, en el orden en que se intentan:
/// Gemini primero (histórico, mismo proveedor que la síntesis de
/// `skill_ingest`), Mistral y Cohere como respaldo automático si
/// Gemini falla o no está configurada.
#[derive(Debug, Clone, Default)]
pub struct EmbeddingKeys {
    /// Clave de la API de Gemini (`GEMINI_API_KEY`).
    pub gemini: Option<String>,
    /// Clave de la API de Mistral (`MISTRAL_API_KEY`).
    pub mistral: Option<String>,
    /// Clave de la API de Cohere (`COHERE_API_KEY`).
    pub cohere: Option<String>,
}

impl EmbeddingKeys {
    /// Lee `GEMINI_API_KEY`, `MISTRAL_API_KEY` y `COHERE_API_KEY` del
    /// entorno. Ninguna es obligatoria; sin ninguna, [`embed_document`]
    /// y [`embed_query`] fallan con [`EmbedError::NoProvider`].
    pub fn from_env() -> Self {
        let key = |var: &str| std::env::var(var).ok().filter(|k| !k.is_empty());
        EmbeddingKeys {
            gemini: key("GEMINI_API_KEY"),
            mistral: key("MISTRAL_API_KEY"),
            cohere: key("COHERE_API_KEY"),
        }
    }

    /// `true` si hay al menos un proveedor configurado.
    pub fn any_configured(&self) -> bool {
        self.gemini.is_some() || self.mistral.is_some() || self.cohere.is_some()
    }
}

/// Un vector junto con el identificador exacto del modelo que lo
/// produjo — ver el comentario de módulo sobre por qué siempre viajan
/// juntos.
#[derive(Debug, Clone, PartialEq)]
pub struct Embedded {
    /// El vector generado.
    pub vector: Vec<f32>,
    /// Identificador estable (`"<proveedor>:<modelo>@<dims>"`) a
    /// persistir junto al vector.
    pub model_id: &'static str,
}

/// Fallo generando un embedding: de red/deserialización con un
/// proveedor concreto, error de la propia API, o ningún proveedor
/// disponible.
#[derive(Debug, thiserror::Error)]
pub enum EmbedError {
    /// Fallo de red o deserialización hablando con `provider`.
    #[error("{provider}: fallo de red o deserialización: {source}")]
    Http {
        /// Nombre corto del proveedor (`"gemini"`, `"mistral"`, `"cohere"`).
        provider: &'static str,
        #[source]
        source: reqwest::Error,
    },
    /// La API de `provider` devolvió una respuesta de error.
    #[error("{provider}: la API devolvió un error: {body}")]
    Api {
        /// Nombre corto del proveedor (`"gemini"`, `"mistral"`, `"cohere"`).
        provider: &'static str,
        /// Cuerpo de la respuesta de error, para depuración.
        body: String,
    },
    /// Ninguna clave de [`EmbeddingKeys`] está configurada.
    #[error("no hay proveedor de embeddings configurado")]
    NoProvider,
    /// Todos los proveedores configurados fallaron; agrega sus motivos.
    #[error("todos los proveedores de embeddings fallaron: {0}")]
    AllFailed(String),
}

// -------------------------------------------------------------------
// Gemini
// -------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct GeminiRequestPart {
    text: String,
}

#[derive(Debug, Serialize)]
struct GeminiRequestContent {
    parts: Vec<GeminiRequestPart>,
}

#[derive(Debug, Serialize)]
struct GeminiRequest {
    model: String,
    content: GeminiRequestContent,
    #[serde(rename = "outputDimensionality")]
    output_dimensionality: usize,
}

#[derive(Debug, Deserialize)]
struct GeminiResponseValue {
    values: Vec<f32>,
}

#[derive(Debug, Deserialize)]
struct GeminiResponse {
    embedding: GeminiResponseValue,
}

async fn embed_gemini(
    client: &reqwest::Client,
    api_key: &str,
    text: &str,
) -> Result<Vec<f32>, EmbedError> {
    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{GEMINI_MODEL}:embedContent?key={api_key}"
    );
    let req_body = GeminiRequest {
        model: format!("models/{GEMINI_MODEL}"),
        content: GeminiRequestContent {
            parts: vec![GeminiRequestPart { text: text.to_string() }],
        },
        output_dimensionality: GEMINI_DIMENSIONS,
    };
    let err_http = |e: reqwest::Error| EmbedError::Http { provider: "gemini", source: e };

    let resp = client.post(&url).json(&req_body).send().await.map_err(err_http)?;
    if !resp.status().is_success() {
        let body = resp.text().await.map_err(err_http)?;
        return Err(EmbedError::Api { provider: "gemini", body });
    }
    let parsed: GeminiResponse = resp.json().await.map_err(err_http)?;
    Ok(parsed.embedding.values)
}

// -------------------------------------------------------------------
// Mistral
// -------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct MistralRequest<'a> {
    model: &'static str,
    input: [&'a str; 1],
}

#[derive(Debug, Deserialize)]
struct MistralEmbeddingData {
    embedding: Vec<f32>,
}

#[derive(Debug, Deserialize)]
struct MistralResponse {
    data: Vec<MistralEmbeddingData>,
}

async fn embed_mistral(
    client: &reqwest::Client,
    api_key: &str,
    text: &str,
) -> Result<Vec<f32>, EmbedError> {
    let err_http = |e: reqwest::Error| EmbedError::Http { provider: "mistral", source: e };
    let resp = client
        .post("https://api.mistral.ai/v1/embeddings")
        .bearer_auth(api_key)
        .json(&MistralRequest { model: MISTRAL_MODEL, input: [text] })
        .send()
        .await
        .map_err(err_http)?;
    if !resp.status().is_success() {
        let body = resp.text().await.map_err(err_http)?;
        return Err(EmbedError::Api { provider: "mistral", body });
    }
    let parsed: MistralResponse = resp.json().await.map_err(err_http)?;
    parsed
        .data
        .into_iter()
        .next()
        .map(|d| d.embedding)
        .ok_or_else(|| EmbedError::Api {
            provider: "mistral",
            body: "la respuesta no trae datos de embedding".to_string(),
        })
}

// -------------------------------------------------------------------
// Cohere
// -------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct CohereRequest<'a> {
    model: &'static str,
    texts: [&'a str; 1],
    input_type: &'static str,
    embedding_types: [&'static str; 1],
}

#[derive(Debug, Deserialize)]
struct CohereEmbeddingsFloat {
    float: Vec<Vec<f32>>,
}

#[derive(Debug, Deserialize)]
struct CohereResponse {
    embeddings: CohereEmbeddingsFloat,
}

async fn embed_cohere(
    client: &reqwest::Client,
    api_key: &str,
    text: &str,
    input_type: &'static str,
) -> Result<Vec<f32>, EmbedError> {
    let err_http = |e: reqwest::Error| EmbedError::Http { provider: "cohere", source: e };
    let resp = client
        .post("https://api.cohere.com/v2/embed")
        .bearer_auth(api_key)
        .json(&CohereRequest {
            model: COHERE_MODEL,
            texts: [text],
            input_type,
            embedding_types: ["float"],
        })
        .send()
        .await
        .map_err(err_http)?;
    if !resp.status().is_success() {
        let body = resp.text().await.map_err(err_http)?;
        return Err(EmbedError::Api { provider: "cohere", body });
    }
    let parsed: CohereResponse = resp.json().await.map_err(err_http)?;
    parsed.embeddings.float.into_iter().next().ok_or_else(|| EmbedError::Api {
        provider: "cohere",
        body: "la respuesta no trae datos de embedding".to_string(),
    })
}

// -------------------------------------------------------------------
// Cadena de respaldo
// -------------------------------------------------------------------

async fn embed_chain(
    client: &reqwest::Client,
    keys: &EmbeddingKeys,
    text: &str,
    cohere_input_type: &'static str,
) -> Result<Embedded, EmbedError> {
    let mut errors = Vec::new();
    if let Some(k) = keys.gemini.as_deref() {
        match embed_gemini(client, k, text).await {
            Ok(vector) => return Ok(Embedded { vector, model_id: GEMINI_MODEL_ID }),
            Err(e) => errors.push(e.to_string()),
        }
    }
    if let Some(k) = keys.mistral.as_deref() {
        match embed_mistral(client, k, text).await {
            Ok(vector) => return Ok(Embedded { vector, model_id: MISTRAL_MODEL_ID }),
            Err(e) => errors.push(e.to_string()),
        }
    }
    if let Some(k) = keys.cohere.as_deref() {
        match embed_cohere(client, k, text, cohere_input_type).await {
            Ok(vector) => return Ok(Embedded { vector, model_id: COHERE_MODEL_ID }),
            Err(e) => errors.push(e.to_string()),
        }
    }
    if errors.is_empty() {
        return Err(EmbedError::NoProvider);
    }
    Err(EmbedError::AllFailed(errors.join(" | ")))
}

/// Embebe un documento a indexar. Gemini y Mistral no distinguen
/// "documento" de "consulta"; Cohere sí, y aquí se le pide
/// `search_document` (ver [`embed_query`] para el otro lado).
pub async fn embed_document(
    client: &reqwest::Client,
    keys: &EmbeddingKeys,
    text: &str,
) -> Result<Embedded, EmbedError> {
    embed_chain(client, keys, text, "search_document").await
}

/// Embebe el texto de una consulta de búsqueda (ver [`embed_document`]
/// sobre por qué existen las dos funciones).
pub async fn embed_query(
    client: &reqwest::Client,
    keys: &EmbeddingKeys,
    text: &str,
) -> Result<Embedded, EmbedError> {
    embed_chain(client, keys, text, "search_query").await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sin_proveedores_falla_claro() {
        let keys = EmbeddingKeys::default();
        assert!(!keys.any_configured());
    }

    #[test]
    fn con_una_clave_se_considera_configurado() {
        let keys = EmbeddingKeys { gemini: Some("x".into()), ..Default::default() };
        assert!(keys.any_configured());
    }
}
