# `wf-browse` may write the wishlist — a narrow, explicit exception to ADR-0003

ADR-0003 forbids `wf-browse` from writing `owned-relics.json` because that
file is OCR-derived ground truth about the game state, and a second,
GUI-editable definition of "owned" would silently diverge from what the
scanner actually observed. The upcoming equipment wishlist (a hand-curated
set of wanted reward parts, stored separately as `wishlist.json`) has no
scan or API source to diverge from — the player's stated intent *is* the
data, there's nothing else it could disagree with. So `wf-browse` is allowed
to write `wishlist.json` directly, while ADR-0003's rule continues to apply
unchanged, and only, to `owned-relics.json` and any future OCR-derived state.
This is a scoped carve-out for one specific file, not a general loosening of
the read-only boundary.
