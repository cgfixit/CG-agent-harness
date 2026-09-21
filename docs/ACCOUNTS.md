# Accounts and roles

Roles, login, and Terminal account commands. Index: [setup-guide.md](../setup-guide.md). Migration and TLS: [SECURE_RESEARCH.md](SECURE_RESEARCH.md).

## 10. Accounts and roles

Fresh homes create restricted `admin` / `admin`; login and replace it immediately
with at least 12 characters. `admin` manages global credentials, users and URL
policy; `operator` uses normal authorized harness work; `audit` sees designated
redacted status/audit information. Every role can change its own password.
Operational routes require login when enabled. Existing config is preserved;
set `auth.enabled: true` to enable this boundary on a legacy home. See [account
migration, shared resources, last-admin protection and recovery](SECURE_RESEARCH.md).

| Account | Portal work | Administrative changes |
|---|---|---|
| Administrator (`admin`) | Chat, permitted research, own sessions/jobs and separately authorized coding | Users, roles, reset/disable, global keys and web policy |
| Portal operator (`operator`) | Same permitted operational resources and coding gates | Own password/logout only |
| Auditor (`audit`) | Minimal status and designated redacted audit entries | Own password/logout only; no chat/research/jobs |

The account bar displays the signed-in role. An Auditor sees a permission refusal
in **Sessions** and a read-only `/status` with minimal details; chat, web, users
and API Keys remain unavailable to that role. This presentation does not grant
additional backend permissions.

Use **change password** in the account bar and provide the current password.
An administrator creates users through **USERS** and can reset another user's
password. Resets, role changes, disabling and deletion revoke that user's sessions.
The last enabled administrator cannot be removed, disabled or demoted. **logout**
revokes this session and clears private displayed state; sign in again to resume.

For Terminal, while this home's server is running:

```bash
./target/release/cgagentharness account login admin
./target/release/cgagentharness account password
./target/release/cgagentharness web status
./target/release/cgagentharness account logout
```

Passwords are read privately from stdin, never arguments. Add `--url <actual-origin>`
after `account` or `web` for a nondefault port, including the desktop's ephemeral
origin. Both clients enforce the same account/CSRF/TLS boundaries as the console.
Backup and recovery must preserve `auth.sqlite3` and `auth.initialized` together;
do not delete them to recreate `admin/admin`. A legacy pending-password account
uses its existing recovery flow, while malformed initialized storage refuses startup.

## Session ownership and legacy adoption

New sessions use schema version 1 and the authenticated account's random `user_id`.
Listing, loading, search, export, chat, prompt preview, goals, skills and style
selection enforce that owner. An administrator does not automatically inherit
another account's sessions. Auth-disabled homes use the explicit `local` owner;
enabling authentication does not silently move that data into a human account.

Sessions without an owner are unassigned legacy shared data. They remain on disk,
are hidden from ordinary reads/search/export, and survive **Clear my session
history**. In **Sessions**, an administrator can choose **Review unassigned legacy
sessions (admin)**, inspect metadata, enter a reason, confirm authorization and
choose **Adopt into my account**. Adoption preserves the transcript and old blob
pin owners but clears the prior goal-stage coding approval. The administrator
must stage and review coding work again. There is no implicit sharing or account
reassignment. Unknown-schema, unreadable and interrupted staged files remain for
manual recovery; clearing history cannot invent their owner.

Detached job and schedule management uses the same owner boundary. Old ownerless
job records remain on disk outside account APIs. Schedule dispatch rechecks the
current account role and disabled/password-change state; revocation stops later
occurrences. Account deletion does not reassign retained records to a newly
created account with the same username.

These boundaries do not provide universal multi-tenant isolation. Persona,
pinned notes, selected model, aggregate spend, the public-page cache, and underlying
agentic run records remain shared among authorized portal operators/admins.
The dedicated machine gateway does not inherit console authority.
