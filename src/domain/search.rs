//! Catalogue search, run on every key. Matching is done by frizbee, the fuzzy
//! matcher of skim, atuin and television (fzf's Smith-Waterman algorithm):
//! the letters of each word of the query must appear in order and close
//! together, so "reviw" finds "review". A word that matches nothing that way
//! is matched again with typos allowed, so "reveiw" finds it too.

use std::borrow::Cow;
use std::cmp::{Ordering, Reverse};

use frizbee::{CaseMatching, Config, Matcher, SortStrategy};

use super::index::Entry;

/// Catalogue order: stars descending, then identity.
pub fn catalog_order(a: &Entry, b: &Entry) -> Ordering {
    b.stars.cmp(&a.stars).then_with(|| a.source.cmp(&b.source))
}

/// Positions of the entries matching `query`, most relevant first: a word
/// found in the name counts most, then in the id, a topic or owner/repo, then
/// in the description; in each field, a word as typed at the start of a word
/// beats one inside a word, which beats letters with gaps. Equally relevant
/// entries keep their order, so the most starred come first. An empty query
/// keeps every entry.
pub fn search<'a>(entries: impl IntoIterator<Item = &'a Entry>, query: &str) -> Vec<usize> {
    let entries: Vec<&Entry> = entries.into_iter().collect();
    let words: Vec<&str> = query.split_whitespace().collect();
    if words.is_empty() {
        return (0..entries.len()).collect();
    }
    let fields = Fields::new(&entries);
    let mut relevance = vec![Some(0); entries.len()];
    for word in words {
        let mut found = fields.best(word, 0);
        if found.iter().all(Option::is_none) {
            found = fields.best(word, typos(word));
        }
        for (total, weight) in relevance.iter_mut().zip(found) {
            *total = total.zip(weight).map(|(total, weight)| total + weight);
        }
    }
    let mut ranked: Vec<(usize, u32)> = relevance
        .into_iter()
        .enumerate()
        .filter_map(|(position, total)| Some((position, total?)))
        .collect();
    ranked.sort_by_key(|&(position, total)| (Reverse(total), position));
    ranked.into_iter().map(|(position, _)| position).collect()
}

#[derive(Debug, Clone, Copy)]
enum Field {
    Name,
    Id,
    Topic,
    Source,
    Description,
}

impl Field {
    fn weight(self) -> u32 {
        match self {
            Self::Name => 3,
            Self::Id | Self::Topic | Self::Source => 2,
            Self::Description => 1,
        }
    }
}

/// How closely a field matched: letters with gaps, the word as typed inside
/// a word, or at the start of one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Closeness {
    Gaps,
    Inside,
    WordStart,
}

/// Every searchable text of the entries, in one list for the matcher.
struct Fields<'a> {
    texts: Vec<Cow<'a, str>>,
    /// Entry position and field of each text.
    owners: Vec<(usize, Field)>,
    entries: usize,
}

impl<'a> Fields<'a> {
    fn new(entries: &[&'a Entry]) -> Self {
        let mut fields = Self {
            texts: Vec::new(),
            owners: Vec::new(),
            entries: entries.len(),
        };
        for (position, entry) in entries.iter().enumerate() {
            let mut push = |text: Cow<'a, str>, field| {
                fields.texts.push(text);
                fields.owners.push((position, field));
            };
            push(Cow::Borrowed(&entry.name), Field::Name);
            push(Cow::Borrowed(&entry.id), Field::Id);
            push(Cow::Owned(entry.source.to_string()), Field::Source);
            for topic in &entry.topics {
                push(Cow::Borrowed(topic), Field::Topic);
            }
            if let Some(description) = &entry.description {
                push(Cow::Borrowed(description), Field::Description);
            }
        }
        fields
    }

    /// Points of the best field of each entry that `word` matches, with up
    /// to `typos` of its letters missing or wrong: the field first, then how
    /// closely it matched.
    fn best(&self, word: &str, typos: u16) -> Vec<Option<u32>> {
        let config = Config::default()
            .casing(CaseMatching::Ignore)
            .max_typos(Some(typos))
            .sort(SortStrategy::IndexAsc);
        let letters = word.chars().count();
        let widest = widest_span(letters, typos);
        let mut best = vec![None; self.entries];
        for found in Matcher::new(word, &config).match_list_indices(&self.texts) {
            // The matcher keeps the best local alignment, which may cover
            // only part of the word: require all its letters but the typos.
            let (Some(&first), Some(&last)) =
                (found.indices.iter().min(), found.indices.iter().max())
            else {
                continue;
            };
            let span = (last - first + 1) as usize;
            if found.indices.len() + usize::from(typos) < letters || span > widest {
                continue;
            }
            let text = &self.texts[found.index as usize];
            let closeness = if span > letters || found.indices.len() < letters {
                Closeness::Gaps
            } else if starts_word(text, first as usize) {
                Closeness::WordStart
            } else {
                Closeness::Inside
            };
            let (entry, field) = self.owners[found.index as usize];
            let points = field.weight() * 3 + closeness as u32;
            best[entry] = best[entry].max(Some(points));
        }
        best
    }
}

/// Whether the byte at `index` begins a word of `text`.
fn starts_word(text: &str, index: usize) -> bool {
    text.get(..index).is_some_and(|before| {
        !before
            .chars()
            .next_back()
            .is_some_and(char::is_alphanumeric)
    })
}

/// Letters of `word` that may be missing or wrong: none under 4 letters,
/// then one per 4 letters.
fn typos(word: &str) -> u16 {
    (word.chars().count() / 4) as u16
}

/// Widest stretch of text a word of `letters` letters may cover: one extra
/// letter per 3, so that letters scattered over a sentence or a long name do
/// not match. A word of 1 or 2 letters must appear as typed.
fn widest_span(letters: usize, typos: u16) -> usize {
    letters + letters / 3 + usize::from(typos)
}
