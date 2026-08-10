# Token-relay session nonce is not a held/transmitted credential

ADR-0001 permits reading process memory ("the optional, unimplemented future
idea of reading inventory via `process_vm_readv` stays a read, permanently")
but separately declined a credential-based login API "because... we will not
hold or transmit account credentials." Phase 4's validated approach
(`docs/research/mobile-inventory-api-coverage.md`,
[issue #52](https://github.com/albrektsson/warframe-lite/issues/52)) reads a
session nonce out of the already-running, already-authenticated game
client's memory and forwards it once to DE's own `inventory.php` endpoint.
We decided this is a *read*, not the credential path ADR-0001 rejected: the
nonce is short-lived, cannot itself be used to log into the game, and is
never persisted — the app never owns a credential lifecycle, it only echoes
back a token the client already holds, once, in the same breath it reads it.

This is recorded as its own ADR rather than an edit to ADR-0001, since
ADR-0001 describes its own rule as "intentionally hard to reverse... treated
as overriding" — kept intact as the permanent record, narrowed here rather
than amended in place.
