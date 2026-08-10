//! Calls DE's own inventory endpoint with a session authz extracted from
//! process memory (see [`crate::process`]) — ADR-0013's "echo the token back
//! once" read, not a held credential. Field parsing is out of scope here
//! (see #57): this only confirms the plumbing works end-to-end and returns
//! the raw response body.

use anyhow::Result;

use crate::process::Authz;

/// Hosts to try, in order. `api.warframe.com` first: research on issue #51
/// found `mobile.warframe.com` can return an empty HTTP 200 for a valid authz
/// while the identical authz succeeds against `api.warframe.com` — so an
/// empty body from a host isn't proof the authz is bad, just a reason to try
/// the next one.
const HOSTS: &[&str] = &["api.warframe.com", "mobile.warframe.com"];

/// Fetch the raw inventory JSON. Single attempt per host, no retry/backoff:
/// walks [`HOSTS`] in order, moving to the next host only on an empty or
/// failed response, and returns the first non-empty body it gets.
pub async fn fetch_inventory(client: &reqwest::Client, authz: &Authz) -> Result<String> {
    let qs = authz.query_string();
    let mut last_err: Option<anyhow::Error> = None;

    for host in HOSTS {
        let url = format!("https://{host}/api/inventory.php{qs}");
        let outcome = async {
            let resp = client.get(&url).send().await?;
            let status = resp.status();
            if !status.is_success() {
                anyhow::bail!("HTTP {status}");
            }
            let body = resp.text().await?;
            anyhow::Ok(body)
        }
        .await;

        match outcome {
            Ok(body) if !body.is_empty() => return Ok(body),
            Ok(_) => {
                tracing::debug!("{host} returned an empty 200 body — trying the next host");
                last_err = Some(anyhow::anyhow!("{host}: empty response body"));
            }
            Err(e) => last_err = Some(e.context(host.to_string())),
        }
    }

    Err(last_err
        .unwrap_or_else(|| anyhow::anyhow!("no host attempted"))
        .context("inventory.php call failed on every host"))
}
