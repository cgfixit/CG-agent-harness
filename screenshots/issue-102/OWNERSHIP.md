# Owner isolation and migration acceptance

Local Safari Computer Use on 2026-09-21 used private browsing, disposable homes,
two synthetic accounts, a synthetic local model, and the actual built Rust server.
No operator credentials or real conversations were used.

| Screenshot | Observed behavior |
| --- | --- |
| [Legacy adoption](legacy-adoption-safari.png) | Unassigned metadata stays outside normal inventory; administrator reviews a reason and explicit adoption confirmation. |
| [Adopted session](adopted-session-safari.png) | The administrator can read the retained transcript after adoption; disk verification showed schema 1, acting owner and cleared goal approval. |
| [Other account](owner-isolation-safari.png) | The operator sees only its own session and receives a legacy-administration refusal. |
| [Clear history](owned-clear-dialog-safari.png) | Confirmation identifies owned history and retention of other accounts and legacy data; Cancel was exercised in the UI. Actual deletion/isolation is covered by guarded HTTP tests. |
| [Logout](logout-clears-context-safari.png) | After selecting a session, setting a synthetic private goal and typing an unsent draft, logout removes the transcript, draft, session ID, goal and loop state. |

The first two screenshots use debug SHA-256
`7b6ee6ffbf5499c4398d21404979a5245f9c7807008abb3aaebbea7ad4288f44`.
The latter three use
`46f9f5b218be1d8142f0e0c233a732760d699e0b8c01361ab563d5427ffa716e`,
which also clears the footer goal/session and loop state on logout.
These are browser/backend observations, not packaged native-app or live-provider
acceptance. They cover owner isolation work, not the unimplemented cron, durable
notification or memory gateway phases of #102.
