//! Cliente mínimo del endpoint `embedContent` de Gemini
//! (`gemini-embedding-001`, truncado a 768 dimensiones).
//!
//! `text-embedding-004` (el modelo original de este proyecto) fue
//! retirado — Google devuelve 404 en `v1beta`. `gemini-embedding-001`
//! es un embedding "Matryoshka": el mismo vector es válido truncado a
//! varias dimensionalidades, seleccionable con `outputDimensionality`
//! en la petición. Fijamos 768 explícitamente porque `schema.sql`
//! define la columna como `vector(768)` — el valor por defecto del
//! modelo es mayor (3072) y Postgres rechazaría la inserción si no lo
//! recortáramos aquí.
//!
//! Un solo lugar para el nombre del modelo: `outbox-worker` (camino de
//! escritura, genera el embedding de un concepto recién commiteado) y
//! `supabase-store` (camino de lectura, genera el embedding de la
//! consulta de búsqueda) DEBEN usar exactamente el mismo modelo Y la
//! misma dimensionalidad — comparar con distancia de coseno vectores
//! de dos modelos, o de dos dimensionalidades, distintos no produce un
//! error, produce un ranking sin ningún significado. Duplicar esta
//! llamada en los dos crates arriesgaría justo eso el día que uno de
//! los dos cambie y el otro no.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

pub const MODEL: &str = "gemini-embedding-001";
pub const DIMENSIONS: usize = 768;

/// Fallo al pedir un embedding: o la red/deserialización (`reqwest`)
/// o una respuesta de error de la propia API de Gemini.
#[derive(Debug, thiserror::Error)]
pub enum EmbedError {
    #[error("fallo de red o deserialización hablando con Gemini: {0}")]
    Http(#[from] reqwest::Error),
    #[error("la API de Gemini devolvió un error: {body}")]
    Api { body: String },
}

#[derive(Debug, Serialize)]
struct EmbeddingRequestPart {
    text: String,
}

#[derive(Debug, Serialize)]
struct EmbeddingRequestContent {
    parts: Vec<EmbeddingRequestPart>,
}

#[derive(Debug, Serialize)]
struct EmbeddingRequest {
    model: String,
    content: EmbeddingRequestContent,
    #[serde(rename = "outputDimensionality")]
    output_dimensionality: usize,
}

#[derive(Debug, Deserialize)]
struct EmbeddingResponseValue {
    values: Vec<f32>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingResponse {
    embedding: EmbeddingResponseValue,
}

/// Pide a Gemini el embedding de `text`. Mismo endpoint tanto para
/// indexar un documento como para una consulta de búsqueda — este
/// modelo de Gemini no distingue "modo documento" de "modo consulta".
pub async fn embed(
    client: &reqwest::Client,
    api_key: &str,
    text: &str,
) -> Result<Vec<f32>, EmbedError> {
    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{MODEL}:embedContent?key={api_key}"
    );

    let req_body = EmbeddingRequest {
        model: format!("models/{MODEL}"),
        content: EmbeddingRequestContent {
            parts: vec![EmbeddingRequestPart { text: text.to_string() }],
        },
        output_dimensionality: DIMENSIONS,
    };

    let resp = client.post(&url).json(&req_body).send().await?;

    if !resp.status().is_success() {
        let err_text = resp.text().await?;
        return Err(EmbedError::Api { body: err_text });
    }

    let embed_resp: EmbeddingResponse = resp.json().await?;
    Ok(embed_resp.embedding.values)
}
