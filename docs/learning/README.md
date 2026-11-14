# docs/learning

Background reading PDFs for the operator (Rust, AI software design). Not
project documentation: agents ignore this folder unless explicitly asked. No new
PDFs may be added; the guard's `DOCS_PDFS` list in
[tests/invariant_guard.rs](../../tests/invariant_guard.rs) only shrinks.

- `AI for code complexity optimization A hybrid perspective.pdf`: paper on
  AI-assisted complexity reduction.
- `Guide_to_AI_Software_Design_From_Con.pdf`: AI software design guide.
- `Intro-to-Rust-from-Python-PowerShell.pdf`: Rust primer for Python and
  PowerShell users.
- `rust_cheat_sheet_a4.pdf`: one-page Rust syntax reference.
- `reminder.txt`: the operator's own study reminder; not guidance.

## Harness web and PDF evidence pilot (November 2026)

[Read the pilot report](pdf-web-evidence-pilot.txt) for the #874 extraction
rerun, answer misses, context and turn times, MiniLM passage ranking, and
other places to try smaller handoffs. The production change repairs PDF
reading order; the two HTML cases are controls.

[Native app: RFC 9309 PDF passage](screenshots/rfc9309-native.jpg) ·
[Firefox: Python HTML passage](screenshots/tomllib-firefox.jpg).
Both captures are cropped to the results.
