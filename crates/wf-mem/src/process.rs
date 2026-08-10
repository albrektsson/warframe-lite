//! Locates the running `Warframe.x64.exe` process and reads its session
//! authz marker (`?accountId=...&nonce=...`) straight out of `/proc/[pid]/mem`
//! — the technique validated in issue #52, reimplemented from scratch here.
//!
//! Read-only per ADR-0001: this module only ever opens `/proc/[pid]/mem` for
//! reading, never writes to it, and never attaches as a tracer — the
//! `PTRACE_MODE_ATTACH` permission check that gates the read is a permission
//! check only, not an actual `ptrace` attach (it never stops the target or
//! sends it a signal).

use std::fs;
use std::io::{Read, Seek, SeekFrom};

use anyhow::{bail, Context, Result};

/// The process name to look for, as it appears in `/proc/[pid]/cmdline`
/// (Warframe under Proton/Wine keeps its Windows executable name; matching
/// on `cmdline` rather than the 15-byte-truncated `/proc/[pid]/comm` finds it
/// reliably regardless of how the Wine loader names the task).
const PROCESS_NAME: &str = "Warframe.x64.exe";

/// Marker literal preceding the account id.
const MARKER_PREFIX: &str = "?accountId=";
/// Literal separating the account id from the nonce.
const NONCE_SEP: &str = "&nonce=";
/// Generous upper bound on a real marker's byte length (prefix + a Mongo
/// ObjectId-shaped 24-hex-char id + separator + a multi-digit nonce), used to
/// size the overlap between memory-scan chunks so a marker straddling a
/// chunk boundary is never missed.
const MARKER_MAX_LEN: usize = 96;

/// Memory is read in fixed-size chunks (rather than one allocation per
/// region) so a single huge reserved region can't force a multi-gigabyte
/// allocation.
const CHUNK_SIZE: usize = 4 << 20;

/// Session authz extracted from process memory: the exact query-string
/// fragment DE's inventory endpoint expects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Authz {
    pub account_id: String,
    pub nonce: String,
}

impl Authz {
    /// `?accountId=<id>&nonce=<nonce>` — appended directly to the
    /// `inventory.php` endpoint.
    pub fn query_string(&self) -> String {
        format!("{MARKER_PREFIX}{}{NONCE_SEP}{}", self.account_id, self.nonce)
    }
}

/// Every currently-running pid whose `/proc/[pid]/cmdline` contains
/// [`PROCESS_NAME`] — under Proton a launcher/preloader task can share the
/// game's cmdline without holding its memory, so callers try each candidate
/// rather than assuming the first is the real game process.
pub fn find_pids() -> Result<Vec<i32>> {
    let mut pids = Vec::new();
    for entry in fs::read_dir("/proc").context("reading /proc")? {
        let Ok(entry) = entry else { continue };
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<i32>() else {
            continue;
        };
        let Ok(cmdline) = fs::read(entry.path().join("cmdline")) else {
            continue;
        };
        if contains_bytes(&cmdline, PROCESS_NAME.as_bytes()) {
            pids.push(pid);
        }
    }
    if pids.is_empty() {
        bail!("{PROCESS_NAME} is not running — start Warframe first, then retry `mem-scan`");
    }
    pids.sort_unstable();
    Ok(pids)
}

/// One readable region of a process's address space, from `/proc/[pid]/maps`.
struct Region {
    start: u64,
    end: u64,
}

/// Parse `/proc/[pid]/maps`, keeping only regions this process may read.
fn readable_regions(pid: i32) -> Result<Vec<Region>> {
    let maps_path = format!("/proc/{pid}/maps");
    let text = fs::read_to_string(&maps_path)
        .with_context(|| format!("reading {maps_path} — is {PROCESS_NAME} still running?"))?;

    let mut regions = Vec::new();
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let (Some(addr_range), Some(perms)) = (fields.next(), fields.next()) else {
            continue;
        };
        if !perms.starts_with('r') {
            continue;
        }
        let Some((start, end)) = addr_range.split_once('-') else { continue };
        let (Ok(start), Ok(end)) = (u64::from_str_radix(start, 16), u64::from_str_radix(end, 16))
        else {
            continue;
        };
        regions.push(Region { start, end });
    }
    Ok(regions)
}

/// A permission-denied open of `/proc/[pid]/mem` gets a clear, actionable
/// error instead of a bare `io::Error` — the caller shouldn't have to guess
/// that it's a Yama `ptrace_scope`/capability issue.
fn permission_hint(e: std::io::Error, pid: i32) -> anyhow::Error {
    if e.kind() == std::io::ErrorKind::PermissionDenied {
        anyhow::anyhow!(
            "permission denied opening /proc/{pid}/mem — same-uid memory reads need either \
             `ptrace_scope <= 1` (check `/proc/sys/kernel/yama/ptrace_scope`) or CAP_SYS_PTRACE \
             granted to this binary (`sudo setcap cap_sys_ptrace=+ep <path-to-wf-lite>`)"
        )
    } else {
        anyhow::anyhow!("opening /proc/{pid}/mem: {e}")
    }
}

/// Scan every readable region of `pid`'s memory for the
/// `?accountId=...&nonce=...` marker and parse the first match into an
/// [`Authz`]. Single linear pass, no retry: a scan that finds nothing is a
/// plain error, not something this function retries on its own.
pub fn scan_authz(pid: i32) -> Result<Authz> {
    let regions = readable_regions(pid)?;
    let mem_path = format!("/proc/{pid}/mem");
    let mut mem = fs::File::open(&mem_path).map_err(|e| permission_hint(e, pid))?;
    let mut buf = vec![0u8; CHUNK_SIZE];

    for region in &regions {
        let mut offset = region.start;
        while offset < region.end {
            let want = ((region.end - offset) as usize).min(CHUNK_SIZE);
            if mem.seek(SeekFrom::Start(offset)).is_err() {
                break;
            }
            // A failed or short read here is an expected, ordinary outcome —
            // Wine's own address space reserves ranges that look mapped in
            // `maps` but aren't fully backed — so this skips ahead rather
            // than treating it as fatal for the whole scan.
            let n = match mem.read(&mut buf[..want]) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            if let Some(authz) = find_marker(&buf[..n]) {
                return Ok(authz);
            }
            let step = n.saturating_sub(MARKER_MAX_LEN).max(1);
            offset += step as u64;
        }
    }

    bail!(
        "scanned {PROCESS_NAME} (pid {pid}) but found no session marker — make sure you're \
         logged in and have loaded into the orbiter or a mission at least once this session"
    )
}

fn contains_bytes(hay: &[u8], needle: &[u8]) -> bool {
    find_bytes(hay, needle).is_some()
}

fn find_bytes(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Find and parse the first `?accountId=<hex>&nonce=<digits>` marker in `buf`.
fn find_marker(buf: &[u8]) -> Option<Authz> {
    let prefix = MARKER_PREFIX.as_bytes();
    let mut search_from = 0;
    while let Some(rel) = find_bytes(&buf[search_from..], prefix) {
        let id_start = search_from + rel + prefix.len();
        if let Some(authz) = parse_marker_at(buf, id_start) {
            return Some(authz);
        }
        search_from = id_start;
    }
    None
}

/// Parse an account id + nonce starting right after a matched `?accountId=`
/// prefix at `id_start`. `None` means this occurrence of the prefix wasn't
/// followed by a well-formed marker (e.g. a coincidental substring) — the
/// caller keeps scanning past it.
fn parse_marker_at(buf: &[u8], id_start: usize) -> Option<Authz> {
    let rest = &buf[id_start..];
    let id_len = rest.iter().position(|b| !b.is_ascii_hexdigit())?;
    if id_len == 0 {
        return None;
    }
    let after_id = &rest[id_len..];
    let sep = NONCE_SEP.as_bytes();
    if !after_id.starts_with(sep) {
        return None;
    }
    let nonce_start = id_len + sep.len();
    let nonce_rest = &rest[nonce_start..];
    let nonce_len = nonce_rest.iter().position(|b| !b.is_ascii_digit()).unwrap_or(nonce_rest.len());
    if nonce_len == 0 {
        return None;
    }

    let account_id = std::str::from_utf8(&rest[..id_len]).ok()?.to_string();
    let nonce = std::str::from_utf8(&nonce_rest[..nonce_len]).ok()?.to_string();
    Some(Authz { account_id, nonce })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_clean_marker() {
        let hay = b"garbage before ?accountId=5f1b2c3d4e5f6a7b8c9d0e1f&nonce=1234567890 garbage after";
        let authz = find_marker(hay).expect("marker found");
        assert_eq!(authz.account_id, "5f1b2c3d4e5f6a7b8c9d0e1f"); //gitleaks:allow
        assert_eq!(authz.nonce, "1234567890");
    }

    #[test]
    fn query_string_round_trips() {
        let authz = Authz { account_id: "abc123".into(), nonce: "42".into() };
        assert_eq!(authz.query_string(), "?accountId=abc123&nonce=42");
    }

    #[test]
    fn ignores_a_prefix_with_no_well_formed_marker_after_it() {
        // The literal appears, but with no nonce following — a coincidental
        // or truncated match should not parse into an `Authz`.
        let hay = b"...?accountId=deadbeef but no nonce here...";
        assert!(find_marker(hay).is_none());
    }

    #[test]
    fn skips_a_malformed_occurrence_and_finds_the_real_one_after_it() {
        let hay = b"?accountId=nothex!!! junk ?accountId=cafebabe&nonce=99 trailing";
        let authz = find_marker(hay).expect("marker found");
        assert_eq!(authz.account_id, "cafebabe");
        assert_eq!(authz.nonce, "99");
    }

    #[test]
    fn finds_nothing_in_unrelated_bytes() {
        assert!(find_marker(b"nothing interesting in here").is_none());
    }
}
