use std::cmp::Ordering;

use super::index::Entry;

/// Catalogue order: stars descending, then identity.
pub fn catalog_order(a: &Entry, b: &Entry) -> Ordering {
    b.stars.cmp(&a.stars).then_with(|| a.source.cmp(&b.source))
}

/// Case-insensitive substring on the name, the id, the description,
/// owner/repo and the topics. An empty query matches everything.
pub fn matches(entry: &Entry, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }
    let contains = |text: &str| text.to_lowercase().contains(&query);
    contains(&entry.name)
        || contains(&entry.id)
        || entry.description.as_deref().is_some_and(contains)
        || contains(&format!("{}/{}", entry.source.owner, entry.source.repo))
        || entry.topics.iter().any(|topic| contains(topic))
}

/// Positions of the matching entries, in their current order. In memory only.
pub fn search(entries: &[Entry], query: &str) -> Vec<usize> {
    entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| matches(entry, query))
        .map(|(index, _)| index)
        .collect()
}
