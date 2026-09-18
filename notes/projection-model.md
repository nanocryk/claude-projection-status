# Projection model

How end-of-window usage is predicted for the 5h and 7d rate-limit windows.

## Two quantities, two units

Consumption happens while working, not while time passes. The model keeps those
apart:

- **intensity** `lambda`: percent of the window's budget per **active hour**
- **exposure** `A`: a number of **active hours**

```
proj = pct + lambda_hat * A_remaining
```

Wall-clock time never multiplies an intensity. It only feeds the profile that
produces `A`. A week holding 45 expected working hours cannot be projected as
168 hours of consumption.

`Pct`, `ActiveHours` and `PctPerActiveHour` are distinct types, so a rate can
only be multiplied by the kind of time it is a rate over.

## Window anchor

A window opens at `resets_at - window_length` (5h = 18000s, 7d = 604800s) with
usage at zero. That anchor is exact and needs no stored history: elapsed window
time and total window time both follow from the payload alone.

## Activity profile

A **slot** is one (local weekday, hour) pair. A slot is *active* when usage
increased during it.

- `occurrences` counts every calendar occurrence of the slot since history
  began, whether or not the tool was running: absence of a sample is evidence of
  inactivity, not absence of evidence.
- Both counters decay by `0.5 ^ (dt / 28 days)`, so the profile follows habits
  as they move.
- `a(h,d) = (active + alpha * m) / (occurrences + alpha)` with `alpha` around 4
  effective occurrences and `m` the pooled parent estimate: the hour-of-day
  marginal, itself pooled toward the global mean.
- Counts are smoothed circularly across neighbouring hours with a `[.25 .5 .25]`
  kernel before the ratio is taken, so a single observed evening cannot pin a
  slot at 0 or 1.
- Slots are local-time: a working day is a local-time thing.
- An hour is judged once, when it closes, and the verdict is stored. Everything
  downstream reads that verdict back: the profile's counters, the active hours
  a window has seen, and the intensity a finished window contributes to its
  prior. One hour therefore cannot be active in one calculation and idle in
  another, and a long session costs no more to read than a short one.
- Usage that appears across a gap between samples (another client, another
  machine) is spread over the gap's slots weighted by `a(h,d)`, rather than
  credited to the slot that happened to hold the next sample.

## Intensity

Usage per active hour is a count-like quantity, so it takes the conjugate
Gamma-Poisson posterior mean:

```
lambda_hat = (k * lambda_prior + used_observed) / (k + A_observed)
```

Both sides of that fraction describe the same stretch of the window: the one
that was watched. `used_observed` is what was spent between the first reading
of this window and now, and `A_observed` is the active hours in that same
stretch. Usage from before the first reading still anchors the projection, as
the point it starts from, but it is not evidence of a pace: budget spent over
an unknown number of working hours says nothing about how fast the work goes.

- `lambda_prior` is the decayed intensity of past windows of the same kind.
- `k` is the prior's weight in active hours, a quarter of the working hours the
  window is expected to hold. Scaling it to the window keeps the prior worth
  the same fraction of a 5h window as of a 7d one; a constant sized for the
  week would swamp the shorter window, which holds only a few active hours.

Minutes into a window, `k` dominates and the projection says "this window is
going like your usual ones". As real data accumulates it takes over. A
near-zero denominator is impossible while `k > 0`, so a fresh window cannot
produce an unbounded projection.

With no history at all, `lambda_prior` is the intensity that spends one
window's budget over one window's working hours (`100% / A_total`). A fresh
window then reads as exactly on budget, and one already ahead of that pace
reads above it. Anchoring on the whole window rather than on what is left
keeps the assumption from growing as the window empties.

## Derived values

All three come from one walk over the window's remaining slots, so they cannot
disagree with each other:

- `A_remaining = sum over remaining slots of a(h,d) * slot_fraction`
- time to 100%: the point in that walk where accumulated usage crosses 100
- confidence: relative width of the posterior, `1 / sqrt(evidence)`, where
  evidence counts only observed budget (the mass behind the prior, plus what
  this window has spent). A cold start's invented prior contributes none, so a
  first run reads as low confidence rather than borrowing certainty from an
  assumption.

The displayed rates come from the same estimate: `%/h` is `lambda_hat`, and
`%/d` is `lambda_hat` times the expected active hours in a day.

Both windows use this estimator, with a prior of their own.
