//! Trade price lookups from warframe.market.
//!
//! Uses the **v2** API (`/v2/orders/item/{slug}`). The legacy v1 endpoint now
//! returns 403, so v2 is the only working option. v2 returns orders under a
//! top-level `data` array, with `type` ("sell"/"buy") and a nested `user`
//! whose `status` is "ingame" | "online" | "offline".

use serde::{Deserialize, Serialize};

const BASE: &str = "https://api.warframe.market/v2";

#[derive(Debug, Deserialize)]
struct OrdersResponse {
    data: Vec<Order>,
}

#[derive(Debug, Deserialize)]
struct Order {
    platinum: u32,
    #[serde(default = "one")]
    quantity: u32,
    /// "sell" or "buy".
    #[serde(rename = "type")]
    order_type: String,
    #[serde(default)]
    visible: bool,
    user: OrderUser,
}

#[derive(Debug, Deserialize)]
struct OrderUser {
    /// "ingame", "online", or "offline".
    status: String,
}

fn one() -> u32 {
    1
}

/// A condensed price summary computed from the live order book.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PriceSummary {
    /// Lowest sell price among sellers who are online or in-game.
    pub lowest_sell: Option<u32>,
    /// Highest buy price among buyers who are online or in-game.
    pub highest_buy: Option<u32>,
    /// Number of active (online/in-game) sell orders considered.
    pub active_sellers: usize,
}

/// A warframe.market client bound to a platform (pc, ps4, xbox, switch).
#[derive(Clone)]
pub struct MarketClient {
    client: reqwest::Client,
    platform: String,
}

impl MarketClient {
    pub fn new(client: reqwest::Client, platform: impl Into<String>) -> Self {
        Self {
            client,
            platform: platform.into(),
        }
    }

    /// Fetch a [`PriceSummary`] for `slug`, warframe.market's identifier for an
    /// item (e.g. `"mirage_prime_set"`).
    pub async fn price_summary(&self, slug: &str) -> anyhow::Result<PriceSummary> {
        let url = format!("{BASE}/orders/item/{slug}");
        tracing::debug!("GET {url}");
        let resp = self
            .client
            .get(&url)
            .header("Platform", self.platform.as_str())
            .header("Language", "en")
            .send()
            .await?
            .error_for_status()?
            .json::<OrdersResponse>()
            .await?;

        Ok(summarize(&resp.data))
    }
}

/// An order counts toward pricing only if it is visible and its owner is
/// reachable for a trade right now.
fn is_active(o: &Order) -> bool {
    o.visible && matches!(o.user.status.as_str(), "ingame" | "online")
}

fn summarize(orders: &[Order]) -> PriceSummary {
    let mut summary = PriceSummary::default();
    for o in orders.iter().filter(|o| is_active(o)) {
        match o.order_type.as_str() {
            "sell" => {
                summary.active_sellers += 1;
                summary.lowest_sell = Some(match summary.lowest_sell {
                    Some(cur) => cur.min(o.platinum),
                    None => o.platinum,
                });
            }
            "buy" => {
                summary.highest_buy = Some(match summary.highest_buy {
                    Some(cur) => cur.max(o.platinum),
                    None => o.platinum,
                });
            }
            _ => {}
        }
        let _ = o.quantity; // reserved for future volume-aware pricing
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order(platinum: u32, order_type: &str, status: &str, visible: bool) -> Order {
        Order {
            platinum,
            quantity: 1,
            order_type: order_type.to_string(),
            visible,
            user: OrderUser {
                status: status.to_string(),
            },
        }
    }

    #[test]
    fn deserializes_v2_shape() {
        // Mirrors the real v2 payload: `data[]` with a `type` field.
        let json = r#"{"apiVersion":"0.25.0","data":[
            {"platinum":99,"quantity":1,"type":"sell","visible":true,
             "user":{"status":"online"}},
            {"platinum":80,"quantity":1,"type":"buy","visible":true,
             "user":{"status":"ingame"}}
        ]}"#;
        let resp: OrdersResponse = serde_json::from_str(json).unwrap();
        let s = summarize(&resp.data);
        assert_eq!(s.lowest_sell, Some(99));
        assert_eq!(s.highest_buy, Some(80));
    }

    #[test]
    fn picks_lowest_active_sell_and_highest_active_buy() {
        let orders = vec![
            order(50, "sell", "ingame", true),
            order(45, "sell", "online", true),
            order(30, "sell", "offline", true),  // excluded: offline
            order(999, "sell", "ingame", false), // excluded: not visible
            order(20, "buy", "online", true),
            order(25, "buy", "ingame", true),
        ];
        let s = summarize(&orders);
        assert_eq!(s.lowest_sell, Some(45));
        assert_eq!(s.highest_buy, Some(25));
        assert_eq!(s.active_sellers, 2);
    }

    #[test]
    fn empty_when_no_active_orders() {
        let orders = vec![order(10, "sell", "offline", true)];
        let s = summarize(&orders);
        assert_eq!(s.lowest_sell, None);
        assert_eq!(s.active_sellers, 0);
    }
}
