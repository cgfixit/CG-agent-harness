# Git approval and publication boundary

The retained clone is data, including its local Git configuration and index.
Approval must commit the bytes and modes that were reviewed. Separate push and
publication must continue to refer to that approved commit.

## Reproduced defects

At `c373c279d1feb89bc4a9c5608c2cdf930176cfb8`, actual harness fixtures reproduced:

- A pre-staged unrelated file entered the approved commit.
- An approved filename containing a literal `*` also staged a matching neighbor.
- `post-checkout` ran despite commit's `--no-verify`; independent local Git probes
  also demonstrated prepare-commit-message, post-commit and pre-push hooks.
- A configured clean filter ran during ordinary staging and diff inspection and
  could substitute index bytes without changing the reviewed worktree bytes.

A candidate-review probe additionally reproduced Git replacement refs changing
the accepted base tree while HEAD still printed its original object ID. The
regression failed against the first candidate, then passed with replacement
interpretation disabled at the shared boundary.

Remote Git trees do not ordinarily install `.git/config` or hooks. Executable
configuration attacks require effective configuration pointing at untrusted
source or contaminated retained metadata. Index/pathspec integrity failures do
not require a hook. These distinctions matter when assessing exposure.

## Behavior

Workspace operations, live manifest reads and disposable-copy verification use
one agentic Git helper. It clears ambient Git variables, global/system config
and system attributes; pins hooks, fsmonitor, signing and automatic maintenance
off; uses literal pathspecs; and disables replacement objects. Local configuration
is restricted to ordinary clone metadata. Includes, custom programs, filters,
transport rewrites and unsupported metadata are refused without echoing values.
Existing read-only diff inspection still works with inert local external-diff
and fsmonitor settings. Auth/CSRF and the server-to-child boundary are unchanged.

Authenticated `gh repo clone` also receives isolated Git settings and an empty
Git template before initial checkout. Publishing Git operations use the installed
`gh auth git-credential` helper rather than ambient Git credential commands.
Custom credential helpers, SSH configurations and enterprise-host arrangements
have not been accepted as equivalent workflows; the selected GitHub.com repository
and absolute local remotes are the supported destinations in this path.

Approval refuses a pre-existing staged change or index lock. It holds the normal
Git index lock, constructs a fresh private index from the accepted base, inserts
only accepted raw blobs with accepted executable modes, and writes that exact
tree. Git replacement objects and graft metadata cannot reinterpret the base.
The acceptance digest now includes mode; older pending records require a new
run. A failure after the branch commit becomes durable is explicitly
indeterminate and requires inspection before retry.

Content-transforming attributes (`filter`, `text`, `eol`, `working-tree-encoding`,
`ident`) are refused for selected changes. This deliberately includes LFS and
newline-normalizing workflows: silently bypassing their transformations could
commit incorrect representations. Supporting them requires a separately reviewed
acceptance contract for both worktree and committed representations.

Run records pin the origin before proposal and retain the approved commit ID.
Push refuses changed local branches or destinations, and sends an object-ID
refspec to the pinned URL. Publication reads the remote branch and refuses a
commit mismatch. Each operation still checks current write policy and its own
reason/confirmation. Older approved records without these pins cannot be pushed;
inspection and cleanup remain available.

## Verification and limits

`tests/git_approval.rs` uses real disposable Git repositories and no model/mock
substitute for Git. It covers unrelated staged changes, literal glob filenames,
hook/filter markers, transformation refusal, index locks, file modes, local and
remote branch drift, destination drift and replacement objects. Existing policy
revocation, safe inspection, CLI/API and local-bare end-to-end tests remain.

The final full native quality run passed 154 tests, formatting, exact clippy and
release build. Dependency policy passed separately. Actual Chrome/Qwen/Cargo
acceptance then passed through complete diff review, exact-tree commit, pinned
local-bare push and reviewed-body mock publication. The standard quality script
explicitly omitted live smoke; the browser/model evidence is separate. No
dependency was added. Exact CI is recorded in the PR and delivery record.

The fresh candidate reviewer confirmed the replacement-ref bypass before its
remaining review was interrupted by a tool-side security filter. That independent
review is incomplete; the parent reproduced the defect and continued local
source review and verification. This is not an exhaustive Git security audit.

The index lock coordinates normal Git writers. No atomic transaction against a
hostile process ignoring locks and racing arbitrary filesystem metadata is
claimed. Remote readback is a point-in-time check; a different authorized actor
can change a branch afterward. Abrupt server death and escaped descendants retain
the documented process-lifecycle limitations. No global Git settings are changed.
