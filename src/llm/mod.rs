//! Local OpenAI-compatible backend resolution and the chat client.

/// Byte ceiling for one model reply: the local JSON body, the local SSE
/// stream and every cloud provider reply stop here, bounding model-controlled
/// bytes independently of the requested tokens.
pub const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

pub mod backend;
pub mod cloud_chat;
pub mod cloud_profile;
pub mod inventory;
pub mod ollama;
pub mod openai_chat;
pub mod openai_stream;
pub mod profile;
pub mod spend;

#[cfg(test)]
mod secret_tests {
    use super::backend::resolve_local_backend;
    use crate::common::config::AppConfig;

    #[tokio::test]
    async fn backend_representations_omit_the_usable_credential() {
        let secret = "synthetic-local-credential";
        let cfg = AppConfig::from_str(
            &format!("models:\n  local_llm:\n    base_url: http://localhost:11434/v1\n    model: fixture\n    api_key: {secret}\n"),
            std::path::Path::new("config.yaml"),
        ).unwrap();
        let backend = resolve_local_backend(&cfg).await.unwrap();
        assert_eq!(backend.api_key, secret, "authentication still receives the key");
        let debug = format!("{backend:?}");
        let json = serde_json::to_value(&backend).unwrap();
        assert!(!debug.contains(secret));
        assert!(!debug.contains("api_key"));
        assert!(json.get("api_key").is_none());
        assert_eq!(json["model"], "fixture");
        assert_eq!(json["provider"], "ollama");
        assert!(debug.contains("fixture"));
    }
}
