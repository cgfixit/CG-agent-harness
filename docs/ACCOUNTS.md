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
| Administrator (`admin`) | Chat, permitted research, shared sessions/jobs and separately authorized coding | Users, roles, reset/disable, global keys and web policy |
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
