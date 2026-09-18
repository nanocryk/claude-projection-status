# Pace

How hard a window is being pushed, as one number, and what is left to spend
once that number goes over.

## Pace

```
pace = intensity * A_remaining / (100 - used)
```

The intensity being kept, against the intensity the remaining budget affords.
It is the projection restated: a pace above 100% is exactly a projection above
100%, so the two cannot disagree. It sits beside the rate.

A ratio has no unit to misread, and it stays finite as a window empties: as the
remaining active hours go to zero, so does the pace. It is undefined only once
the budget is gone, where there is nothing left to pace and the figure is
dropped.

## What is left, once the pace is over 100%

The two windows answer that question in the unit that suits their length, and
each carries one of the two, so a line never holds two time-shaped numbers at
once. Both appear only while the pace is over 100%.

The 5h window gives the work the budget still buys:

```
work_remaining = (100 - used) / intensity
```

Hours of work, not hours on the clock. It moves only while work is happening:
idle time spends nothing, so it subtracts nothing. Speeding up shortens it,
since it is budget over the pace being kept. Five hours hold no night, so a
calendar deadline over that span would answer nearly the same question in a
form that reads as a countdown.

The 7d window gives the moment the limit lands, as a local weekday and hour:
`Mon 18h`. Over a week the two figures part company, because the walk to the
limit spends only in the hours the profile expects to be worked, and nights and
weekends push the date well past the hours of work behind it. Written as a date
rather than as a span, it cannot be read as a second countdown.
