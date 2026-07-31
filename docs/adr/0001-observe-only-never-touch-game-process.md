# Observe-only: never write to or interact with the Warframe process

warframe-lite exists to cover AlecaFrame's most-used features without
Overwolf, and AlecaFrame's own approach — reading (and, for some features,
touching) the game via a native memory-access plugin — was available to copy.
We deliberately did not: the app may only *read* two one-directional,
game-unaware channels (the `EE.log` file, and the game window's pixels via
X11 `GetImage`). It must never send input, write to process memory, attach as
a debugger/tracer, or send the game any IPC, network traffic, or signals —
even the optional, unimplemented future idea of reading inventory via
`process_vm_readv` stays a *read*, permanently.

This also ruled out the alternative of a credential-based login API for full
inventory data (declined in favor of the memory-reading idea specifically
*because* it's read-only, despite being unstarted and higher-effort) — we
will not hold or transmit account credentials.

The constraint is intentionally hard to reverse: it is treated as
overriding any feature request, not a default that can be relaxed for a
compelling use case.
