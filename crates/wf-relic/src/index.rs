//! Fuzzy resolution of an OCR'd reward name to a catalogue [`Item`].
//!
//! OCR of Warframe's stylised font is imperfect (dropped/!added letters, icon
//! noise), so exact matching is useless. We normalise both sides to lowercase
//! alphanumerics and pick the catalogue name with the smallest Levenshtein
//! distance, accepting it only above a similarity threshold.

use wf_data::items::{fetch_items, Item};

/// A resolved match with its similarity score (1.0 = identical).
#[derive(Debug, Clone)]
pub struct Match<'a> {
    pub item: &'a Item,
    pub score: f32,
}

/// The item catalogue with precomputed normalised names for fast matching.
pub struct ItemIndex {
    items: Vec<Item>,
    normalized: Vec<String>,
}

impl ItemIndex {
    /// Build an index from catalogue items.
    pub fn new(items: Vec<Item>) -> Self {
        let normalized = items.iter().map(|i| normalize(&i.name)).collect();
        Self { items, normalized }
    }

    /// Fetch the catalogue and build an index.
    pub async fn load(client: &reqwest::Client) -> anyhow::Result<Self> {
        Ok(Self::new(fetch_items(client).await?))
    }

    /// Build the index from a disk cache when fresh (younger than `ttl`),
    /// otherwise refetch and update the cache. On a failed refetch, fall back to
    /// a stale cache if one exists.
    pub async fn load_cached(
        client: &reqwest::Client,
        ttl: std::time::Duration,
    ) -> anyhow::Result<Self> {
        const FILE: &str = "items.json";
        if let Some(cached) = wf_cache::load_blob::<Vec<Item>>(FILE) {
            if cached.age() < ttl {
                tracing::info!("item catalogue from cache ({} items)", cached.value.len());
                return Ok(Self::new(cached.value));
            }
            // Stale: try to refresh, but keep the stale copy if the network fails.
            match fetch_items(client).await {
                Ok(items) => {
                    let _ = wf_cache::save_blob(FILE, &items);
                    tracing::info!("item catalogue refreshed ({} items)", items.len());
                    return Ok(Self::new(items));
                }
                Err(e) => {
                    tracing::warn!("catalogue refresh failed ({e}); using stale cache");
                    return Ok(Self::new(cached.value));
                }
            }
        }
        let items = fetch_items(client).await?;
        let _ = wf_cache::save_blob(FILE, &items);
        Ok(Self::new(items))
    }

    /// Number of catalogue entries.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Best fuzzy match for `query`, or `None` if nothing clears the threshold.
    pub fn best_match(&self, query: &str) -> Option<Match<'_>> {
        let q = normalize(query);
        if q.is_empty() {
            return None;
        }
        let qb = q.as_bytes();

        let mut best: Option<(usize, usize)> = None; // (index, distance)
        for (i, name) in self.normalized.iter().enumerate() {
            // Cheap length-difference prune before the full edit-distance.
            let len_diff = name.len().abs_diff(q.len());
            if let Some((_, best_dist)) = best {
                if len_diff > best_dist {
                    continue;
                }
            }
            let d = levenshtein(qb, name.as_bytes());
            if best.is_none_or(|(_, bd)| d < bd) {
                best = Some((i, d));
                if d == 0 {
                    break;
                }
            }
        }

        let (idx, dist) = best?;
        let longest = self.normalized[idx].len().max(q.len()).max(1);
        let score = 1.0 - dist as f32 / longest as f32;
        // A fairly strict threshold: reward-screen names crop cleanly and should
        // match their exact catalogue name at ~0.85+, so this rejects
        // confident-but-wrong matches (e.g. untradable rewards like Forma that
        // are absent from the catalogue) without losing real, noisy matches.
        if score >= 0.8 {
            Some(Match {
                item: &self.items[idx],
                score,
            })
        } else {
            None
        }
    }
}

/// Lowercase and strip to ASCII alphanumerics.
pub fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Classic Wagner–Fischer Levenshtein distance over byte slices.
pub fn levenshtein(a: &[u8], b: &[u8]) -> usize {
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0usize; b.len() + 1];
    for (i, &ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            curr[j + 1] = (prev[j + 1] + 1).min(curr[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str, ducats: Option<u32>) -> Item {
        Item {
            slug: normalize(name),
            name: name.to_string(),
            ducats,
            tags: vec![],
        }
    }

    fn sample_index() -> ItemIndex {
        ItemIndex::new(vec![
            item("Mirage Prime Blueprint", Some(100)),
            item("Mirage Prime Systems Blueprint", Some(45)),
            item("Forma Blueprint", None),
            item("Volt Prime Neuroptics Blueprint", Some(45)),
        ])
    }

    #[test]
    fn matches_despite_ocr_noise() {
        let idx = sample_index();
        // OCR dropped a letter and added icon noise.
        let m = idx.best_match("M1RAGE PRIME BLUEPR1NT ~").unwrap();
        assert_eq!(m.item.name, "Mirage Prime Blueprint");
    }

    #[test]
    fn distinguishes_similar_names() {
        let idx = sample_index();
        // Real reward screens show the full name, including "Blueprint".
        let m = idx.best_match("MIRAGE PRIME SYSTEMS BLUEPRINT").unwrap();
        assert_eq!(m.item.name, "Mirage Prime Systems Blueprint");
    }

    #[test]
    fn rejects_garbage() {
        let idx = sample_index();
        assert!(idx.best_match("zzzzqwxk").is_none());
    }

    #[test]
    fn rejects_untradable_lookalike() {
        // "Forma Blueprint" is not in a prime-only catalogue; it must not match a
        // similar prime blueprint above threshold.
        let idx = ItemIndex::new(vec![item("Bo Prime Blueprint", Some(15))]);
        assert!(idx.best_match("Forma Blueprint").is_none());
    }

    #[test]
    fn levenshtein_basics() {
        assert_eq!(levenshtein(b"kitten", b"sitting"), 3);
        assert_eq!(levenshtein(b"", b"abc"), 3);
        assert_eq!(levenshtein(b"abc", b"abc"), 0);
    }
}
