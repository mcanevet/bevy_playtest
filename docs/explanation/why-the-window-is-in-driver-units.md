# Why the window is in driver units

The `stagnant_resource` window (`window_s`) is consumed in the
**driver leaf's own unit**, never converted to wall seconds — and
until the 0.15 line the loader pretended otherwise. This page records
the trap, the fix, and what a scenario author must know when
migrating.

## The trap

`window_s` was validated at load as if it were a duration in
**seconds**: any window larger than `duration_s` was rejected as
"can never elapse." But the runtime budget is consumed by the raw
delta of the driver leaf (`budget += driver_delta`) — a driver
advancing +1 per FixedUpdate tick burns the window in *ticks*.

For a tick-driven game (say 20 ticks/s in a 20-second run that
accumulates 400 ticks):

- A window sized honestly in the author's mental unit — "flag the
  economy if it rests more than 300 ticks" — **rejected at load**,
  with a message claiming the window "can never elapse" when it
  elapses at tick 300, comfortably inside the run.
- The largest window that loaded was 19 — 19 *ticks*, under one
  second of activity demand against a ~2-second harvest cadence.
  It fired nine times on a healthy, live economy.

The validation silently assumed every driver advances at ≥ 1 unit
per second. Tick counters violate it in one direction; accelerated
match clocks (counting in-game seconds faster than wall time)
violate it in the other. Both were mis-served by the same line.

## The fix

The author's clock domain is authoritative; the validation
boundary moved to match it:

- `window_s` is documented as **driver-leaf units**. The
  duration-coupled load rejection is **gone** unless the author
  declares the driver's rate explicitly.
- **Optional `driver_units_per_second`**: when declared (finite,
  > 0), the loader checks the fit arithmetic
  `window_s / rate ≤ duration_s` and rejects with a numerically
  true message — "window (300 driver units at declared 20
  units/s = 15.0s) exceeds the run duration (20s)". The declaration
  doubles as documentation of the driver's unit. Two invariants
  sharing one `(driver_resource, driver_path)` pair must declare
  the same rate, or the scenario rejects (one driver leaf has one
  rate).
- **Post-run elision audit (`window_unelapsed`)**: with or without a
  declared rate, if the driver's total observed advance over the
  whole run never reached the window, the stagnation verdict was
  unjudgeable *for that run* — reported as an **informational**
  note naming the observed advance ("the driver advanced only
  123.0 units over the whole run; lengthen the run or shrink the
  window"). Informational, never a failure: a driver that
  legitimately rests or runs slower than assumed is game behavior
  (the frozen-driver doctrine — a frozen driver freezes the window
  and legitimately holds the invariant's peace).
- `stagnant_resource` rejects `eventually_s` at load, so the
  elision audit never interacts with deadlines.

## Migration

| Old behavior | New behavior |
|---|---|
| `window_s > duration_s` rejects at load | Loads and runs (unless `driver_units_per_second` is declared and the fit check fails) |
| Windows sized in ticks were unrepresentable | Size them in ticks: `window_s: 300` with `driver_units_per_second: 20` documents and validates the intent at load |
| A window too big for the driver was indistinguishable from a pass | `window_unelapsed` informational note names the observed advance |

If your driver counts literal seconds, nothing changes: declare
`driver_units_per_second: 1.0` if you want the load-time check,
or omit it and rely on the post-run audit.

## The same trap, oracle errors, and one more hatch

Three sibling fixes ship with the same doctrine (the validation
boundary must tell the truth about what it knows):

- **`oracle_error` counting**: a static authoring failure (a typo'd
  path, an unknown resource) is a property of the *scenario*, not
  of time — it is now reported **once per distinct error class**
  with `count: 1`, and recurrence folded into the detail
  ("present every frame, frames 0–299; counted once") instead of
  once per frame.
- **The noun fix**: `resource_check` path errors now say "does not
  resolve on this **resource**"; the shared formatter threads the
  noun from the caller instead of hardcoding "component".
- **`"passive": true`** on the bot: an explicit opt-in for
  observe-only runs (no inputs at all — empty input lists are
  otherwise rejected as likely typos). The accidental-typo guard
  stays; intentional observation becomes expressible.
