//! Bounded embeddings client for the research corpus index.
//!
//! The endpoint is validated loopback-only in [`super::ResearchConfig`]; the
//! client itself additionally disables proxies and redirects, caps the
//! response body, and never echoes provider text in an error.

use serde_json::{json, Value};

use crate::common::errors::{HarnessError, Result};

use super::{ResearchConfig, MAX_DIMENSIONS};

/// Any embeddings failure (transport, HTTP status, malformed body) surfaces
/// under this code so the search route can fail closed with one mapping.
pub const EMBED_FAILED: &str = "RESEARCH_EMBED_FAILED";
/// Largest embeddings response body accepted (64 MiB).
const MAX_EMBED_BODY_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone)]
pub struct EmbedClient {
    base: String,
    model: String,
    client: reqwest::Client,
}

impl EmbedClient {
    pub fn new(config: &ResearchConfig) -> Result<Self> {
        let client = crate::llm::ollama::http_client(std::time::Duration::from_secs(config.embeddings_timeout_sec))
            .map_err(|e| HarnessError::new(EMBED_FAILED, e.message))?;
        Ok(Self {
            base: config.embeddings_url.trim_end_matches('/').to_string(),
            model: config.embeddings_model.clone(),
            client,
        })
    }

    /// One batch of embeddings, in input order.
    pub async fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
        let url = format!("{}/embeddings", self.base);
        let request = self
            .client
            .post(&url)
            .json(&json!({"model": self.model, "input": inputs}));
        match crate::llm::ollama::bounded_json(request, MAX_EMBED_BODY_BYTES).await {
            crate::llm::ollama::Bounded::Json(value) => parse_embeddings(&value, inputs.len()),
            crate::llm::ollama::Bounded::Status(status) => Err(HarnessError::new(
                EMBED_FAILED,
                format!("the embeddings endpoint answered HTTP {status}"),
            )),
            crate::llm::ollama::Bounded::Failed => Err(HarnessError::new(
                EMBED_FAILED,
                "the embeddings endpoint was unreachable or answered with an unreadable body",
            )),
        }
    }
}

/// Validate and extract the `data` array of an OpenAI-compatible embeddings
/// response. Every entry must carry a numeric `embedding` of 1..=MAX_DIMENSIONS
/// elements, and the count must match the request exactly.
pub fn parse_embeddings(value: &Value, expected: usize) -> Result<Vec<Vec<f32>>> {
    let data = value
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| HarnessError::new(EMBED_FAILED, "the embeddings response carried no data array"))?;
    if data.len() != expected {
        return Err(HarnessError::new(
            EMBED_FAILED,
            "the embeddings response count does not match the request",
        ));
    }
    let mut out = Vec::with_capacity(expected);
    for entry in data {
        let row = entry
            .get("embedding")
            .and_then(Value::as_array)
            .ok_or_else(|| HarnessError::new(EMBED_FAILED, "an embeddings entry carried no vector"))?;
        if row.is_empty() || row.len() > MAX_DIMENSIONS {
            return Err(HarnessError::new(
                EMBED_FAILED,
                "an embeddings vector is empty or beyond the dimension ceiling",
            ));
        }
        let mut vector = Vec::with_capacity(row.len());
        for x in row {
            let Some(n) = x.as_f64() else {
                return Err(HarnessError::new(
                    EMBED_FAILED,
                    "an embeddings vector held a non-numeric element",
                ));
            };
            vector.push(n as f32);
        }
        out.push(vector);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_well_formed_batch() {
        let value = json!({"data": [{"embedding": [1.0, 0.0]}, {"embedding": [0.0, 1.0]}]});
        let vectors = parse_embeddings(&value, 2).expect("vectors");
        assert_eq!(vectors, vec![vec![1.0, 0.0], vec![0.0, 1.0]]);
    }

    #[test]
    fn rejects_malformed_responses_without_echoing_them() {
        let secret = "provider-secret-text";
        for value in [
            json!({}),
            json!({"data": secret}),
            json!({"data": []}),
            json!({"data": [{"embedding": []}]}),
            json!({"data": [{"embedding": ["x", 1.0]}]}),
            json!({"data": [{"embedding": [1.0]}, {"embedding": [2.0]}]}),
        ] {
            let err = parse_embeddings(&value, 1).expect_err("malformed");
            assert!(err.is(EMBED_FAILED));
            assert!(!err.message.contains(secret));
        }
    }

    #[test]
    fn rejects_oversized_dimensions() {
        let row: Vec<f64> = vec![0.0; MAX_DIMENSIONS + 1];
        let value = json!({"data": [{"embedding": row}]});
        assert!(parse_embeddings(&value, 1).is_err());
        let row: Vec<f64> = vec![0.0; MAX_DIMENSIONS];
        let value = json!({"data": [{"embedding": row}]});
        assert!(parse_embeddings(&value, 1).is_ok());
    }
}
