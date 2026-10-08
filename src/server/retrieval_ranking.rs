use std::cell::OnceCell;
use std::collections::{BTreeMap, BTreeSet};

const CHUNK_CHARS: usize = 1200;
const MAX_TERMS: usize = 32;
pub(super) const CANDIDATE_LIMIT: usize = 64;
const HIT_CAP: usize = 16;

pub(super) struct PreparedQuery {
    pub expression: String,
    /// One exact, word-bounded pattern per identifier-shaped word.
    identifiers: Vec<regex::Regex>,
    lowercase: String,
}

/// Tantivy query text: the whole query as a boosted phrase plus each
/// alphanumeric term quoted. `None` when the query has no terms.
pub(super) fn tantivy_expression(query: &str) -> Option<String> {
    let terms: Vec<_> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .take(MAX_TERMS)
        .collect();
    if terms.is_empty() {
        return None;
    }
    let quoted = query.replace('\\', "\\\\").replace('"', "\\\"");
    Some(format!(
        "\"{quoted}\"^2 {}",
        terms.iter().map(|t| format!("\"{t}\"")).collect::<Vec<_>>().join(" ")
    ))
}

impl PreparedQuery {
    pub fn new(query: &str) -> std::result::Result<Option<Self>, regex::Error> {
        let Some(expression) = tantivy_expression(query) else {
            return Ok(None);
        };
        // Only the identifier-shaped words (`widget_open`, `Vec::new`) must match
        // exactly. A whole-query pattern made `How does widget_open fail?` require
        // that literal sentence, so a question about an identifier found nothing.
        // Tokens are cut on identifier syntax, so `foo_bar/baz_qux` and
        // `` `foo_bar`,`baz_qux` `` are two identifiers, not one literal.
        static IDENTIFIER: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
            regex::Regex::new(r"[\p{L}\p{N}_]+(?:::[\p{L}\p{N}_]+)*").expect("static identifier pattern")
        });
        let identifiers = IDENTIFIER
            .find_iter(query)
            .map(|m| m.as_str())
            .filter(|word| word.contains('_') || word.contains("::"))
            .take(MAX_TERMS)
            .map(|word| {
                // `::` continues a path, so `foo::bar` does not match inside
                // `foo::bar::baz` or `outer::foo::bar`; a lone `:` still ends it.
                regex::RegexBuilder::new(&format!(
                    r"(?:^|[^\p{{L}}\p{{N}}_:]|(?:^|[^:]):){}(?:$|[^\p{{L}}\p{{N}}_:]|:(?:$|[^:]))",
                    regex::escape(word)
                ))
                .case_insensitive(true)
                .build()
            })
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(Some(Self {
            expression,
            identifiers,
            lowercase: query.to_lowercase(),
        }))
    }

    /// At least one identifier in the query appears exactly in some field. Each
    /// passage is checked alone, and docs describe `Vec::new` and
    /// `Vec::with_capacity` in separate chunks, so one is enough per passage.
    pub fn identifier_matches<'a>(&self, fields: impl IntoIterator<Item = &'a str>) -> bool {
        let fields: Vec<&str> = fields.into_iter().collect();
        self.identifiers.is_empty()
            || self
                .identifiers
                .iter()
                .any(|identifier| fields.iter().any(|field| identifier.is_match(field)))
    }

    pub fn exact_bonus(&self, text: &str) -> f32 {
        if text.to_lowercase().contains(&self.lowercase) {
            2.0
        } else {
            0.0
        }
    }
}

pub(super) struct RankedCandidate<T> {
    item: T,
    score: f32,
    tie_key: String,
    diversity_key: String,
}

impl<T> RankedCandidate<T> {
    pub fn new(item: T, score: f32, tie_key: String, diversity_key: String) -> Self {
        Self {
            item,
            score,
            tie_key,
            diversity_key,
        }
    }
}

pub(super) fn select_diverse<T: Clone>(
    mut candidates: Vec<RankedCandidate<T>>,
    limit: usize,
    text: impl Fn(&T) -> &str,
) -> Vec<T> {
    candidates.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.tie_key.cmp(&b.tie_key)));
    let mut selected = Vec::new();
    let normalized: Vec<OnceCell<String>> = (0..candidates.len()).map(|_| OnceCell::new()).collect();
    let hashes: Vec<OnceCell<String>> = (0..candidates.len()).map(|_| OnceCell::new()).collect();
    let words: Vec<OnceCell<BTreeSet<String>>> = (0..candidates.len()).map(|_| OnceCell::new()).collect();
    let mut selected_words: Vec<usize> = Vec::new();
    let mut seen_text = BTreeSet::new();
    let mut source_counts: BTreeMap<String, usize> = BTreeMap::new();
    let limit = limit.min(HIT_CAP);
    for cap in [1, 2] {
        for (candidate_index, candidate) in candidates.iter().enumerate() {
            if selected.len() >= limit {
                break;
            }
            if source_counts.get(&candidate.diversity_key).copied().unwrap_or(0) >= cap {
                continue;
            }
            let current_normalized = normalized[candidate_index]
                .get_or_init(|| text(&candidate.item).split_whitespace().collect::<Vec<_>>().join(" "));
            let hash = hashes[candidate_index].get_or_init(|| crate::common::sha256_hex(current_normalized));
            if seen_text.contains(hash)
                || selected_words.iter().any(|&old_index| {
                    let current = words[candidate_index]
                        .get_or_init(|| current_normalized.split_whitespace().map(str::to_string).collect());
                    let old = words[old_index].get_or_init(|| {
                        normalized[old_index]
                            .get_or_init(|| {
                                text(&candidates[old_index].item)
                                    .split_whitespace()
                                    .collect::<Vec<_>>()
                                    .join(" ")
                            })
                            .split_whitespace()
                            .map(str::to_string)
                            .collect()
                    });
                    !current.is_empty() && current.intersection(old).count() * 10 > current.union(old).count() * 9
                })
            {
                continue;
            }
            seen_text.insert(hash.clone());
            *source_counts.entry(candidate.diversity_key.clone()).or_default() += 1;
            selected_words.push(candidate_index);
            selected.push(candidate.item.clone());
        }
    }
    selected
}

pub(super) fn chunk_text(text: &str) -> Vec<(usize, usize, String, String)> {
    let mut output = Vec::new();
    let mut start = 0;
    let mut heading = String::new();
    while start < text.len() {
        let tail = &text[start..];
        let mut len = tail
            .char_indices()
            .nth(CHUNK_CHARS)
            .map(|(i, _)| i)
            .unwrap_or(tail.len());
        if len < tail.len() {
            if let Some(end) = tail[..len].rfind(['\n', ' ']).filter(|n| *n > len / 2) {
                len = end + 1;
            }
        }
        let end = start + len;
        let chunk = &text[start..end];
        let label = |line: &str| line.trim_start_matches("# ").trim().to_string();
        let chunk_heading = chunk.lines().find(|line| line.starts_with("# ")).map(label);
        if let Some(line) = chunk.lines().rev().find(|line| line.starts_with("# ")) {
            heading = label(line);
        }
        if !chunk.trim().is_empty() {
            output.push((
                start,
                end,
                chunk_heading.unwrap_or_else(|| heading.clone()),
                chunk.to_string(),
            ));
        }
        start = end;
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct Item {
        id: String,
        text: String,
    }

    fn candidate(id: &str, source: &str, score: f32, text: &str) -> RankedCandidate<Item> {
        RankedCandidate::new(
            Item {
                id: id.into(),
                text: text.into(),
            },
            score,
            id.into(),
            source.into(),
        )
    }

    #[test]
    fn overlap_threshold_is_strictly_greater_than_ninety_percent() {
        let ten = "a b c d e f g h i j";
        let exact_ninety = "a b c d e f g h i";
        let over_ninety = "a b c d e f g h i j k";
        let selected = select_diverse(
            vec![
                candidate("base", "a", 3.0, ten),
                candidate("exact", "b", 2.0, exact_ninety),
                candidate("over", "c", 1.0, over_ninety),
            ],
            16,
            |item| &item.text,
        );
        assert_eq!(
            selected.iter().map(|item| item.id.as_str()).collect::<Vec<_>>(),
            ["base", "exact"]
        );
    }

    #[test]
    fn diversity_allows_two_per_source() {
        let candidates = vec![
            candidate("a1", "a", 30.0, "shared a one"),
            candidate("a2", "a", 29.0, "shared a two"),
            candidate("a3", "a", 28.0, "shared a three"),
            candidate("b1", "b", 27.0, "shared b one"),
        ];
        let selected = select_diverse(candidates, 16, |item| &item.text);
        assert_eq!(
            selected.iter().map(|item| item.id.as_str()).collect::<Vec<_>>(),
            ["a1", "b1", "a2"]
        );
    }

    #[test]
    fn total_hits_are_capped_at_sixteen() {
        let mut candidates = Vec::new();
        for i in 0..20 {
            candidates.push(candidate(
                &format!("s{i}"),
                &format!("s{i}"),
                20.0 - i as f32,
                &format!("shared unique marker {i}"),
            ));
        }
        let selected = select_diverse(candidates, 100, |item| &item.text);
        assert_eq!(selected.len(), 16);
    }

    #[test]
    fn identifier_words_in_a_question_match_exactly_without_requiring_the_question() {
        let q = PreparedQuery::new("How does `widget_open` fail?").unwrap().unwrap();
        assert!(q.identifier_matches(["widget_open returns EBUSY on a second open."]));
        assert!(!q.identifier_matches(["widget_opener is unrelated."]));
        assert!(!q.identifier_matches(["How does it fail?"]));
        let both = PreparedQuery::new("compare Vec::new and Vec::with_capacity")
            .unwrap()
            .unwrap();
        assert!(both.identifier_matches(["Vec::new allocates nothing.", "Vec::with_capacity reserves."]));
        // Punctuation between identifiers separates them.
        for joined in ["compare `foo_bar`,`baz_qux`", "foo_bar/baz_qux"] {
            let q = PreparedQuery::new(joined).unwrap().unwrap();
            assert!(
                q.identifier_matches(["foo_bar is set.", "baz_qux is read."]),
                "{joined}"
            );
        }
        // A chunk that documents one of the queried identifiers is kept.
        assert!(both.identifier_matches(["Vec::new allocates nothing."]));
        assert!(!both.identifier_matches(["Vec::newer is unrelated."]));
        let path = PreparedQuery::new("foo::bar").unwrap().unwrap();
        assert!(path.identifier_matches(["Call foo::bar: it returns."]));
        assert!(!path.identifier_matches(["foo::bar::baz and outer::foo::bar"]));
        // A query without an identifier filters nothing.
        assert!(PreparedQuery::new("retry count")
            .unwrap()
            .unwrap()
            .identifier_matches(["x"]));
    }
}
