# Tutorial 2 — Editing: the Vegas verbs

**Goal:** assemble a session with the mouse in under two minutes (the M1
acceptance drill), then fix it with undo.

| Action | How |
| --- | --- |
| Move event | drag its body |
| Trim head/tail | drag the event's left/right edge |
| Slip content (keep position, change what plays) | alt-drag the body |
| Split at playhead | select + `S` |
| Delete + close the gap | select + `Delete` (ripple toggle in the toolbar) |
| Select many | ctrl-click / shift-click |
| Undo / redo | `Ctrl+Z` / `Ctrl+Y` |
| Zoom | ctrl+wheel; zoom-to-selection from the toolbar |

Crossfades: overlap two events on one track and put fades on the overlap
edges — the mixer sums them, so fade-out + fade-in over the overlap is the
classic A/B crossfade.

Everything above is a reversible `EditOp` — the same ops the CLI and the
scripting surface use, so anything you do by hand can be scripted or undone
exactly.

Next: [03-fx-and-automation.md](03-fx-and-automation.md).
