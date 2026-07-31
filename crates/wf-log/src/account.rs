//! Best-effort extraction of the local player's Warframe account id (a 24-hex
//! Mongo ObjectId) from `EE.log`.
//!
//! There is no single authoritative "this is my account id" line: the id only
//! shows up in situational contexts (Duviri races, Void Fissure reward relaying,
//! event challenges), and squadmates' ids appear in the same file. So candidates
//! are ranked by how strongly the line ties them to the local player, and the
//! caller is expected to confirm the top candidate against the public profile API
//! (its `DisplayName` must match the logged-in name). The uppercase matchmaking
//! GUID (`mm=...`) is a different id space and is excluded here by only accepting
//! lowercase hex runs of exactly 24 characters.

use crate::{event_from_line, Event};

/// Result of scanning an `EE.log` for the local account id.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct AccountScan {
    /// Local display name from the last `Logged in <name>` line, if present.
    pub local_name: Option<String>,
    /// Candidate account ids, most-likely first, de-duplicated.
    pub candidates: Vec<String>,
}

/// Scan whole `EE.log` `text` for the local account id (see module docs). The
/// returned candidates are ranked; verify the first that the profile API confirms
/// belongs to `local_name`.
pub fn scan_account(text: &str) -> AccountScan {
    let mut local_name = None;
    for line in text.lines() {
        if let Some(Event::LoggedIn(name)) = event_from_line(line) {
            local_name = Some(name);
        }
    }

    let mut candidates: Vec<String> = Vec::new();
    let push = |ids: Vec<&str>, cands: &mut Vec<String>| {
        for id in ids {
            if !cands.iter().any(|c| c == id) {
                cands.push(id.to_string());
            }
        }
    };

    // Rank 1: any line that also names the local player. The uppercase `mm=`
    // matchmaking GUID on squad lines is excluded by `object_ids` (lowercase-only),
    // leaving the true account id from lines like "... for <name> ... and ID: <id>".
    if let Some(name) = &local_name {
        for line in text.lines().filter(|l| l.contains(name.as_str())) {
            push(object_ids(line), &mut candidates);
        }
    }

    // Rank 2: Void Fissure reward relaying the local client does for itself.
    for line in text.lines().filter(|l| {
        l.contains("VoidProjections:")
            && (l.contains("to host!") || l.contains("Client got reward info from"))
    }) {
        push(object_ids(line), &mut candidates);
    }

    // Rank 3: the host entry of a per-player info update.
    for line in text
        .lines()
        .filter(|l| l.contains("Updated playerInfo for player") && l.contains("Host=true"))
    {
        push(object_ids(line), &mut candidates);
    }

    AccountScan { local_name, candidates }
}

/// Extract maximal lowercase-hex runs of exactly 24 characters (Mongo ObjectIds),
/// skipping the all-zero null id. Longer or uppercase runs are ignored, so the
/// 32-char / uppercase ids that also appear in the log don't leak in.
fn object_ids(s: &str) -> Vec<&str> {
    let bytes = s.as_bytes();
    let is_hex = |b: u8| b.is_ascii_digit() || (b'a'..=b'f').contains(&b);
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if is_hex(bytes[i]) {
            let start = i;
            while i < bytes.len() && is_hex(bytes[i]) {
                i += 1;
            }
            let run = &s[start..i];
            if run.len() == 24 && run != "000000000000000000000000" {
                out.push(run);
            }
        } else {
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
18.927 Sys [Info]: Logged in SpiroTris
100.0 Script [Info]: AddSquadMember: SpiroTris, mm=A61B1C07C521800BCD27FB41, squadCount=1
2308.0 Script [Info]: DuviriAutonomousHorseTimeTrial.lua: Race started for SpiroTris with avatar: ErsatzHorseSummonAvatar223 and ID: 59e4e3f83ade7ff432d804be
3862.0 Net [Info]: New request from 683f2493fcc7c33df60750d1: 172.238.122.1
5112.0 Sys [Info]: VoidProjections: Sending reward info from 59e4e3f83ade7ff432d804be to host!";

    #[test]
    fn finds_local_name_and_id() {
        let scan = scan_account(SAMPLE);
        assert_eq!(scan.local_name.as_deref(), Some("SpiroTris"));
        // The name-correlated id ranks first; the remote peer id is not first.
        assert_eq!(scan.candidates.first().map(String::as_str), Some("59e4e3f83ade7ff432d804be"));
    }

    #[test]
    fn excludes_uppercase_matchmaking_guid() {
        // The `mm=A61B...` GUID is uppercase → never a candidate.
        let scan = scan_account(SAMPLE);
        assert!(!scan.candidates.iter().any(|c| c.eq_ignore_ascii_case("A61B1C07C521800BCD27FB41")));
    }

    #[test]
    fn object_ids_ignores_wrong_length() {
        assert!(object_ids("deadbeef").is_empty());
        assert_eq!(object_ids("x 59e4e3f83ade7ff432d804be y"), vec!["59e4e3f83ade7ff432d804be"]);
        // 25 hex chars → not a 24-run.
        assert!(object_ids("59e4e3f83ade7ff432d804bee").is_empty());
    }

    #[test]
    fn empty_when_no_login() {
        let scan = scan_account("1.0 Sys [Info]: nothing useful here");
        assert_eq!(scan.local_name, None);
        assert!(scan.candidates.is_empty());
    }
}
