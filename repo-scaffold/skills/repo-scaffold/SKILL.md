---
name: repo-scaffold
description: Use when creating a repository or upgrading its scaffolding — agent context files, line-ending normalization, format/lint CI, container CVE scanning, tag-triggered publishing, release automation, and generated release notes. Encodes the working set from maintained repos, CLAUDE.md importing AGENTS.md via the @-syntax (a plain link does not load), .gitattributes eol=lf (the Windows-CI "Delete ␍" fix), prettier+eslint as a read-only CI gate, a weekly Trivy scan tuned to actionable findings, tag globs as a coarse filter backed by a mandatory in-job semver check, prerelease-safe latest, the verified release-please adoption path (token-cascade dispatch, the Actions-may-create-PRs setting, concurrency, issues-write), and label-categorized release notes.
---

# Repo scaffolding that pays rent

Six pieces of scaffolding, each of which has already paid for itself in a maintained repo.
Copy the shapes; the rationale is what keeps them from being cargo cult.

## 1. Agent context: `AGENTS.md`, imported by `CLAUDE.md`

Repo conventions (architecture, invariants, workflow rules) live in **`AGENTS.md`** — the
vendor-neutral file that multiple coding agents read. `CLAUDE.md` then contains exactly one
line:

```text
@AGENTS.md
```

The `@` **import syntax matters**: it inlines the whole file into the agent's context at
session start. A plain markdown link (`See [AGENTS.md](AGENTS.md).`) is *not* followed
automatically — observed side by side in two live repos: the `@`-import version loaded the
full conventions, the link version loaded only the one-line stub. If your conventions aren't
reaching the agent, check which form the repo uses.

## 2. `.gitattributes`: one line ends a whole failure class

```gitattributes
* text=auto eol=lf
```

Prettier and eslint enforce LF. Without this line, Windows CI runners (and contributors with
`core.autocrlf`) check out CRLF and `prettier --check` fails on **every line** with
``Delete `␍` `` — a wall of red unrelated to the change. Related Windows-runner trap: npm
script globs must be double-quoted (`"dist/test/*.test.js"`) or Windows expands them.

## 3. Format locally, verify in CI

Two package.json scripts, one writing, one read-only:

- `format` — `prettier --write . && eslint --fix` (the developer's command)
- `ci-lint` — `eslint && prettier --check .` (what CI runs — read-only, so uncommitted
  format drift fails the build instead of being silently "fixed")

Wire `ci-lint` into the PR workflow of every repo, including ones that started as pure
docs/shell — retrofitting it later means a noisy reformat commit. After bumping prettier /
eslint / typescript versions, run `npm install` before `format`: formatter output diverges
between versions and CI will reject stale-toolchain output.

## 4. Container repos: a weekly Trivy scan, tuned to be actionable

For any repo that publishes a container image, add a scheduled CVE scan (shape from a live
workflow):

- **Weekly cron + `workflow_dispatch`, not per-PR** — freshly disclosed CVEs surface without
  any code change, and per-PR would add an image build (often QEMU) to every PR.
- Build the amd64 image with `push: false, load: true`, scan with the Trivy action, output
  SARIF, upload to the repo's Security → Code scanning tab.
- **Tune for signal**: `ignore-unfixed: true` (CVEs with no fixed version are noise you
  cannot act on) and `severity: HIGH,CRITICAL`. Informational — it gates nothing; a finding
  in a hand-pinned binary is the cue to bump that version ARG.
- Hygiene: guard the job with `if: github.repository == '<owner>/<repo>'` so forks don't
  burn their minutes, and `persist-credentials: false` on checkout when no later step needs
  the token.

## 5. Publishing is tag-triggered CI — never a laptop

- **npm packages: OIDC trusted publishing.** No `NPM_TOKEN` secret, no OTP in CI — the
  registry trusts the workflow identity. The job needs `permissions: contents: read` +
  `id-token: write` (OIDC is dead without the latter). One wrinkle: a *new* package needs
  one manual CLI+OTP publish before a trusted publisher can be configured for it.
- **Container images: GHCR via `GITHUB_TOKEN`** — job permissions `contents: read` +
  `packages: write` — with the details that bite:
  - **The tag glob is a coarse filter, not validation.** Narrow it —
    `"v[0-9]*.[0-9]*.[0-9]*"` beats a bare `v*`, which fires on `vnext` or `vendor-fix` — but
    do not mistake it for a semver check. GitHub's filter syntax is glob, not regex: `[0-9]`
    matches one digit and the following `*` matches *anything*, so each component is "a digit
    then whatever". `v1x.2y.3z`, `v1abc.2def.3ghi` and `v1.2.3.4` all match this "tight"
    pattern. It only buys you the leading-digit-per-component shape; `v1-keeper-final` is
    excluded, `v1zzz.0zzz.0zzz` is not.
  - **So validate in the job — this step is mandatory, not belt-and-braces.** The glob cannot
    anchor and `workflow_dispatch` inputs are free text, so the only real gate is an explicit
    check that fails the job before anything is published:

    ```yaml
    - name: Validate version is strict semver
      run: |
        VERSION="${GITHUB_REF_NAME#v}"
        # MAJOR.MINOR.PATCH with optional -prerelease and +build (semver.org BNF)
        if ! printf '%s' "$VERSION" | grep -Eq '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$'; then
          echo "::error::Tag '$GITHUB_REF_NAME' is not strict semver - refusing to publish."
          exit 1
        fi
        echo "version=$VERSION" >> "$GITHUB_OUTPUT"
    ```

    Note the `(0|[1-9][0-9]*)` components: they reject leading zeros (`v01.2.3`) as semver
    requires, which a naive `[0-9]+` would let through.
  - **Prereleases never move `:latest`**: any version with a prerelease suffix (`-beta.1`,
    `-rc.2`, `-alpha.1`, …) publishes `:VERSION` only — key the check on "has a prerelease
    part", not on an enumerated list, or the next suffix style slips through.
  - Create the GitHub Release in a separate job with `needs: publish`, so a failed image
    push can't leave a Release pointing at an image that never reached the registry. Derive
    `prerelease:` from the tag name.
- **Action pinning is a real tradeoff — pick one and apply it repo-wide.** A version tag
  (`actions/checkout@v6`) is *mutable*: the tag can be repointed at a new commit, so it is a
  trust decision about the publisher, not a cryptographic guarantee. Only a full commit SHA
  (`actions/checkout@<40-char-sha> # v6`) is immutable, and Dependabot updates SHA pins in
  place when that trailing version comment is kept.
  This scaffold defaults to **version tags** for first-party (`actions/*`, `docker/*`) and
  established third-party actions: readable diffs, no churn, and the same style across every
  workflow in the repo. The cost is accepting the publisher's tag hygiene.
  Choose **SHA pins** when the workflow's blast radius justifies it — anything holding
  `id-token: write` (OIDC publish), `packages: write` (registry push), or `actions: write`,
  and any less-established action — or when org policy mandates it. Both publish workflows
  above are in that category, so this is a live choice, not a formality.
  Whichever you pick, be consistent: a lone SHA-pinned step among tagged ones is noise, and
  mixed styles make scanner findings unreadable. Treat a scanner's blanket "pin to SHA"
  finding as this policy question, not a defect.

### Or automate the whole ritual: release-please (verified end-to-end)

Instead of hand-cutting `chore(release): X.Y.Z` PRs and tags:
`googleapis/release-please-action@v5` (`release-type: node`) on every push to the default
branch maintains a **standing Release PR** from the conventional commits — version bump in
`package.json` *and* the lockfile, generated changelog, compare/PR/commit links. Merging that
PR creates the tag and the GitHub Release, so releases still gate on a human merge. Verified
in production (a real version shipped through the full chain); **four things bit on adoption**:

- **Tags pushed with `GITHUB_TOKEN` never trigger your tag-based publish workflow** (GitHub's
  recursion guard). No PAT needed: make the release-please workflow dispatch the publish
  workflow explicitly — `gh workflow run publish.yml --ref "$TAG" -f tag="${TAG#v}"` —
  because `workflow_dispatch` is the documented exemption to the guard. Needs
  `actions: write`; dispatching *at the tag ref* builds the tagged tree even if the default
  branch has moved on. Keep the publish workflow's own Release job gated on push events so
  release-please's Release stays the only one.
- **The repo setting "Allow GitHub Actions to create and approve pull requests" is off by
  default** — the first run does all its branch work and then fails with exactly that
  message. Flip it under Settings → Actions → General (or
  `gh api -X PUT repos/<owner>/<repo>/actions/permissions/workflow -F can_approve_pull_request_reviews=true`).
- **Serialize with a `concurrency` group** (`cancel-in-progress: false`) — every default-branch
  push runs the workflow, and back-to-back merges race over the same Release PR (observed
  immediately: three merges, three simultaneous runs).
- **Permissions**: `contents: write` and `pull-requests: write` on the release-please job.
  Add `issues: write` if a run fails creating its `autorelease:*` labels — it reaches for the
  issues API to make them, so the need shows up on a repo where they don't exist yet rather
  than on every repo.

Taxonomy shifts to be aware of: by default notes come from **commit types**, not PR labels;
`docs` commits are hidden; a `feat` of *any* scope drives a minor — steer an off-policy bump
with an empty commit carrying a `Release-As: X.Y.Z` footer, or pin the whole policy with
`versioning` (`always-bump-patch` makes every release a PATCH regardless of commit type).

#### Credit contributors in the notes, and drop `CHANGELOG.md` entirely

Two config lines change the output more than anything above:

```json
{
  "release-type": "node",
  "versioning": "always-bump-patch",
  "changelog-type": "github",
  "skip-changelog": true,
  "pull-request-title-pattern": "chore: release ${version}",
  "packages": { ".": {} }
}
```

- **`changelog-type: github`** hands note generation to GitHub's own API, which is what
  produces the **`by @author`** credit lines. Beware `include-commit-authors`: it looks like
  the option for this and is a **no-op** — `parseConventionalCommits` rebuilds each commit
  without the `author` field, so the notes never see it
  ([release-please#2892](https://github.com/googleapis/release-please/issues/2892), open as of
  September 2026).
- **It also re-activates `.github/release.yml`** (section 6): with GitHub generating the notes,
  your label categories apply again. Exclude release-please's own PR or the notes list
  themselves — `autorelease: pending` and `autorelease: tagged` alongside `skip-changelog`.
  Because the categories are label-driven, pair this with a small workflow that labels each PR
  from its conventional-commit title, or CLI-opened PRs arrive unlabelled and land in "Other".
- **`skip-changelog: true`** keeps no `CHANGELOG.md` at all — the Releases page is the
  changelog. Worth it beyond taste: the generated file trips Prettier, and CI does not run on
  the release PR, so nothing surfaces the failure.

#### Gate the run, or every docs merge proposes a release

release-please has a "no user-facing commits" skip, but **it never fires under
`changelog-type: github`**: GitHub's generated notes are never empty — they always end in a
*Full Changelog* line — so a lone `docs:` or `ci:` merge still opens a release PR.

Put a `gate` job in front that reads the push payload and only lets release-please run when the
push carries something users get — `feat`/`fix`/`perf`/`revert`, any `type!:`, a
`BREAKING CHANGE:` footer, `build(deps):` (a *runtime* bump; Dependabot scopes dev ones
`deps-dev`), or the release PR's own merge:

```yaml
gate:
  runs-on: ubuntu-24.04
  outputs:
    releasable: ${{ steps.check.outputs.releasable }}
  steps:
    - id: check
      env:
        MESSAGES: ${{ toJSON(github.event.commits.*.message) }}
        RELEASABLE: '^((feat|fix|perf|revert)(\([^)]*\))?!?: |[a-z]+(\([^)]*\))?!: |build\(deps\): |Revert |chore(\([^)]*\))?: release v?[0-9])'
      run: |
        if jq -e --arg re "$RELEASABLE" '(length >= 2048) or ([.[] | (split("\n")[0] | test($re)) or test("\nBREAKING[- ]CHANGE: ")] | any)' <<< "$MESSAGES" > /dev/null; then
          echo "releasable=true" >> "$GITHUB_OUTPUT"
        else
          echo "releasable=false" >> "$GITHUB_OUTPUT"
        fi
```

Match the last alternative to your `pull-request-title-pattern` so the release PR's own merge
is always releasable — that merge is what creates the tag. **Fail open** on a full payload
(`length >= 2048`): a push that large may hide a releasable commit, so let it through rather
than silently skipping a release.

## 6. Release notes are generated, never hand-written

`generate_release_notes: true` on the Release step plus `.github/release.yml` to categorize
the merged PRs by label:

```yaml
changelog:
  exclude:
    labels: [skip-changelog]
  categories:
    - title: 🚀 Features
      labels: [feature, enhancement]
    - title: 🐛 Fixes
      labels: [bug, fix]
    - title: 📦 Dependencies
      labels: [dependencies]
    - title: Other
      labels: ["*"]
```

The notes are built from **PR titles** — which is the operational reason PR titles must
describe the change ("if someone only read the title, would they understand what this
does?"). No hand-maintained CHANGELOG file; it drifts and duplicates the Releases page.

This file stays in play under release-please when you set `changelog-type: github` — same
categories, plus the `autorelease:*` excludes — so keep PRs labelled.

---

*All shapes lifted from live, maintained repos (verified 2026-08-04): the `.gitattributes`
line and its Windows-CI rationale, the format/ci-lint script pair, the weekly Trivy workflow
(cron, SARIF, ignore-unfixed, HIGH/CRITICAL), the dotted tag glob + semver validation +
prerelease-safe `:latest` + `needs:`-gated Release, and the label-categorized `release.yml`.
The `@AGENTS.md` import-vs-link behavior observed live in two repos side by side. The
release-please flow verified end-to-end on a production container repo (2026-08-06): Release
PR → human merge → tag + Release → dispatched publish at the tag ref → multi-arch registry
manifest with `:VERSION` + `:latest`; all four adoption gotchas above were hit and resolved
in that run, not copied from documentation. The `changelog-type: github` contributor lines,
`skip-changelog`, and the releasable-commit gate verified on two npm plugin repos
(2026-09-21); the gate's judgement observed correctly skipping a `ci:` merge in production.
`include-commit-authors` re-confirmed a no-op against release-please#2892, still open.*
