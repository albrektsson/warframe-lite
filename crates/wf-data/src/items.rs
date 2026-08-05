//! The warframe.market item catalogue (v2 `/v2/items`).
//!
//! One request returns every tradable item with its slug, English name, ducat
//! value, and tags — enough to resolve an OCR'd reward name to a market slug and
//! its ducat worth without any per-item calls.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

const BASE: &str = "https://api.warframe.market/v2";

/// A catalogue entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    /// warframe.market slug, e.g. "mirage_prime_blueprint".
    pub slug: String,
    /// English display name, e.g. "Mirage Prime Blueprint".
    pub name: String,
    /// Ducat value (prime parts only).
    pub ducats: Option<u32>,
    /// Tags such as "prime", "component", "blueprint".
    pub tags: Vec<String>,
    /// Whether the item is vaulted (permanently removed from active drop
    /// tables).
    #[serde(default)]
    pub vaulted: bool,
}

#[derive(Deserialize)]
struct ItemsResponse {
    data: Vec<RawItem>,
}

#[derive(Deserialize)]
struct RawItem {
    slug: String,
    #[serde(default)]
    ducats: Option<u32>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    vaulted: bool,
    #[serde(default)]
    i18n: HashMap<String, RawI18n>,
}

#[derive(Deserialize)]
struct RawI18n {
    name: String,
}

impl RawItem {
    fn into_item(mut self) -> Option<Item> {
        // Prefer English; fall back to any available localisation.
        let name = self
            .i18n
            .remove("en")
            .or_else(|| self.i18n.drain().next().map(|(_, v)| v))?
            .name;
        Some(Item {
            slug: self.slug,
            name,
            ducats: self.ducats,
            tags: self.tags,
            vaulted: self.vaulted,
        })
    }
}

/// Fetch the full item catalogue.
pub async fn fetch_items(client: &reqwest::Client) -> anyhow::Result<Vec<Item>> {
    let url = format!("{BASE}/items");
    tracing::debug!("GET {url}");
    let resp = client
        .get(&url)
        .header("Language", "en")
        .send()
        .await?
        .error_for_status()?
        .json::<ItemsResponse>()
        .await?;
    Ok(resp.data.into_iter().filter_map(RawItem::into_item).collect())
}
