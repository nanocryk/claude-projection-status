# The fourth line

A line of no operational value: one fact drawn from the history the tool
already keeps, or one phrase of the reader's own.

## When it shows

`fun_line` takes three values:

- `start`: only before the session's first assistant turn. Claude is open and
  no work has happened yet, so the line costs a terminal row only while there
  is nothing else to look at. This is the default.
- `always`: on every refresh.
- `never`: not drawn.

A transcript that cannot be read counts as not having started, which is the
harmless direction: a joke shows where it might not have.

## What it shows

The content changes with a five-minute bucket of the clock, alternating between
a computed fact and a phrase from `fun_phrases`. Whichever side is empty falls
through to the other, and with both empty the line is not drawn at all.

Binding the choice to the clock rather than to chance is what keeps it
readable: the status line refreshes every ten seconds, and a phrase drawn at
random would never sit still long enough to finish reading.

## The facts

Every one is computed from what the database already holds: the hour-by-hour
verdicts, the activity profile, the sessions table, finished windows, and the
model mix of the session in front of you. None of them is actionable, and that
is the point.

They fall in three registers: guilt (how much of the week has gone into this),
vanity (records worth a small nod), and accounting (where the tokens went). A
fact that has nothing to say stays quiet, so a young database simply shows
fewer of them.
