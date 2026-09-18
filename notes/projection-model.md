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
- Usage that appears across a gap between samples (another client, another
  machine) is spread over the gap's slots weighted by `a(h,d)`, rather than
  credited to the slot that happened to hold the next sample.

## Intensity

Usage per active hour is a count-like quantity, so it takes the conjugate
Gamma-Poisson posterior mean:

```
lambda_hat = (k * lambda_prior + pct) / (k + A_elapsed)
```

- `A_elapsed` is the active hours observed so far in this window.
- `lambda_prior` is the decayed intensity of past windows of the same kind.
- `k` is the prior's weight, expressed in active hours (about one working day).

Minutes into a window, `k` dominates and the projection says "this window is
going like your usual ones". By the time real data accumulates, it dominates
instead. A near-zero denominator is impossible while `k > 0`, so a fresh window
cannot produce an unbounded projection.

With no history at all, `lambda_prior` is budget-neutral (`100% / A_total`) and
the projection reads as exactly on budget, at low confidence.

## Derived values

All three come from one walk over the window's remaining slots, so they cannot
disagree with each other:

- `A_remaining = sum over remaining slots of a(h,d) * slot_fraction`
- time to 100%: the point in that walk where accumulated usage crosses 100
- confidence: relative width of the posterior, `1 / sqrt(shape)`

The displayed rates come from the same estimate: `%/h` is `lambda_hat`, and
`%/d` is `lambda_hat` times the expected active hours in a day.

Both windows use this estimator, with a prior of their own.
