# Clean-Room & Trademark Policy

**Read this before writing any code or documentation for OpenTrack.**
Contributing means you agree to this policy. It exists to keep OpenTrack a
legally clean, independent implementation and to protect every contributor.

---

## 1. What "clean-room" means here

OpenTrack reimplements a *workflow* — a set of publicly observable editing
concepts (unlimited tracks, drag-anywhere events, crossfades on overlap,
per-track buses, takes) — from scratch in Rust. It does **not** reimplement any
proprietary product.

Concretely, this project:

- ✅ MAY study publicly available documentation, marketing material, manuals,
  forum posts, and videos that describe how the workflow behaves.
- ✅ MAY write RFCs that specify behavior in our own words, from scratch.
- ✅ MAY reference trademarks to describe compatibility (nominative fair use),
  per §4.
- ✅ MAY compose with **open-source, GPL-compatible Rust components** whose
  own provenance is clean (e.g. `filmcraft-h264` and `craft-media`, both
  MIT OR Apache-2.0, written from public codec standards — see
  [`ATTRIBUTION.md`](ATTRIBUTION.md)). Attribution and license notices are
  mandatory; proprietary code is still absolutely excluded.
- ❌ MUST NOT decompile, disassemble, or reverse engineer any proprietary
  binary, DLL, installer, or resource file.
- ❌ MUST NOT copy, translate, or port source code from any proprietary or
 GPL-incompatible codebase into this repository.
- ❌ MUST NOT copy asset files (icons, sounds, sample projects, UI imagery)
  from any proprietary product.
- ❌ MUST NOT accept a contribution whose author had exposure to proprietary
  NLE source code for the feature they contribute (see §3 disclosure rule).

## 2. How we work: RFC before feature

Behavior that could be influenced by proprietary knowledge is specified in an
RFC (`docs/rfc/`) **before** implementation:

1. The RFC describes the behavior from public knowledge: what a user sees and
   does, in our own words.
2. Reviewers check the RFC for compliance with this policy (see checklist).
3. Only after merge does implementation begin.

If you already know how a competitor implements something internally, you may
still contribute — but only to features specified by someone else's RFC, and
your knowledge must not leak into the implementation (see §3).

### Reviewer checklist

- [ ] Does the RFC cite only public sources (manuals, videos, forums)?
- [ ] Is every statement phrased as our own specification, not a copy of
      documentation text?
- [ ] Is there any sign of memory-layout, protocol, or file-format details
      that could only come from decompilation?
- [ ] For file-format claims (e.g., `.veg` import): are they limited to
      publicly documented, community-verified observations?

## 3. Contributor disclosure rule

By opening a PR you declare that the code is yours, that it is original, and
that you are not under any agreement (employment, NDA, or otherwise) that
prohibits this contribution. If you have ever had authorized access to
proprietary NLE source code, you must state it in your first PR so maintainers
can route you to non-adjacent work. We would rather lose a contribution than
contaminate the clean-room.

## 4. Trademark policy

- "Vegas", "Sony Vegas", "VEGAS Pro" and "Magix" are trademarks of their
  respective owners (Sony Creative Software Inc. / MAGIX Software GmbH).
- This project's name is **OpenTrack**. It must never be styled, prefixed or
  suffixed in a way that suggests endorsement by, or affiliation with, the
  trademark owners.
- The word "Vegas" may appear **only** in explanatory contexts such as
  "inspired by the classic Vegas-era editing workflow" or "workflow parity
  with Vegas" — nominative fair use to describe what we are *like*, never as
  a claim that we *are* that product or are compatible with its file formats
  beyond what public community documentation establishes.
- Binary releases, installers, and website copy get an explicit disclaimer
  (see the README footer).

## 5. License

OpenTrack is **GPL-3.0-or-later** (see [`LICENSE`](LICENSE)). All
contributions are accepted under the same license, with a DCO-style
`Signed-off-by` line attesting origin (see
[`CONTRIBUTING.md`](CONTRIBUTING.md)).

## 6. If a rights holder objects

Maintainers will respond to any substantiated takedown request within 7 days:
remove the disputed material, audit history, and publish what changed. Report
concerns to the maintainers privately first (see CONTRIBUTING.md) before
opening a public issue.
