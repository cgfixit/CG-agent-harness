//! One bounded DNS lookup for every pinned egress client: web fetch and
//! search, MCP SSE, completion webhooks and the local model resolver. Each
//! caller keeps its own address policy (public-only, loopback-only, or the
//! webhook's private opt-in); this module only bounds how much of an answer
//! anyone collects.

use std::net::SocketAddr;

/// Most addresses an egress client pins. A larger answer is refused by the
/// caller, never truncated into a partial pin set.
pub const MAX_PINNED_ADDRESSES: usize = 32;

/// Resolve `host:port`, keeping at most `MAX_PINNED_ADDRESSES + 1` answers:
/// the extra one lets a caller tell an oversized answer from a full one
/// without collecting the rest.
pub async fn lookup_bounded(host: &str, port: u16) -> std::io::Result<Vec<SocketAddr>> {
    Ok(tokio::net::lookup_host((host, port))
        .await?
        .take(MAX_PINNED_ADDRESSES + 1)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_literal_address_resolves_to_itself_within_the_bound() {
        let answers = lookup_bounded("127.0.0.1", 8443).await.unwrap();
        assert_eq!(answers, vec!["127.0.0.1:8443".parse::<SocketAddr>().unwrap()]);
        assert!(answers.len() <= MAX_PINNED_ADDRESSES);
    }
}
