# Releasing & CI notes

How releases work and the non-obvious gotchas we hit setting them up. Read this
before touching `release.yml`, `docker.yml`, or `dist-workspace.toml`.

## How to cut a release

1. Bump `version` in `Cargo.toml` (and let `Cargo.lock` sync).
2. Commit to `main`.
3. Tag it and push the tag:
   ```sh
   git tag v0.1.6
   git push origin v0.1.6
   ```
4. Both workflows trigger **only on the tag**:
   - **`release.yml`** (cargo-dist) builds binaries for all 4 platforms and
     publishes a GitHub Release with installers (`curl … | sh`).
   - **`docker.yml`** builds the multi-arch image and pushes to GHCR.

A normal release (code changes only) publishes automatically. No token, no
manual step.

## Gotcha 1 — `release.yml` is hand-edited (`--target` removed)

**Symptom:** the `host` job fails at *"Create GitHub Release"* with
`HTTP 403: Resource not accessible by integration`, even though the repo has
"Read and write permissions" and the token shows `Contents: write`.

**Cause:** `gh release create <tag> --target <commit>` makes GitHub evaluate the
commit range from `<commit>` to HEAD. If that range touches any
`.github/workflows/*` file, GitHub requires the token to also have
`workflows: write` — a scope the default `GITHUB_TOKEN` **cannot** be granted
(it's rejected as invalid in the YAML). This is an intentional GitHub security
rule (Nov 2023). It bites specifically when the release being cut *contains
workflow changes*, which is why a normal code-only release is fine.

Refs: [cli/cli#9514](https://github.com/cli/cli/issues/9514),
[GitHub changelog](https://github.blog/changelog/2023-11-02-github-actions-enforcing-workflow-scope-when-creating-a-release/).

**Fix applied:** removed `--target "$RELEASE_COMMIT"` from the `gh release
create` line in `release.yml`. The tag already exists when the workflow runs
(the tag is what triggers it), so `--target` is redundant — the release lands on
the tag's commit regardless. No token needed.

## Gotcha 2 — `allow-dirty = ["ci"]` in `dist-workspace.toml`

cargo-dist's `host` step verifies that `release.yml` matches what
`dist generate` would produce and aborts with exit 255 ("out of date contents")
otherwise. Because we hand-removed `--target`, we set `allow-dirty = ["ci"]` so
dist keeps our customization instead of rejecting it.

**⚠️ If you ever run `dist generate` again** (e.g. after changing
`dist-workspace.toml`), it will regenerate `release.yml` and **put `--target`
back**. You must remove it again (see Gotcha 1). Verify with:
```sh
grep "gh release create" .github/workflows/release.yml   # must NOT contain --target
dist generate --check                                    # must exit 0
```

## Gotcha 3 — runner images must be current

cargo-dist 0.22.1 defaulted to retired runner images (`ubuntu-20.04`,
`windows-2019`, `macos-13`); jobs queued forever with no runner assigned.
Fixed by upgrading to dist 0.32.0 (current defaults: `ubuntu-22.04`,
`windows-2022`, `macos-14`/`macos-15-intel`). Keep `cargo-dist-version` current.

## Gotcha 4 — Docker arm64 must build on a native runner

`docker.yml` builds `linux/amd64` and `linux/arm64`. Building arm64 under QEMU
emulation on an amd64 runner compiles Rust (incl. `aws-lc-rs`) emulated and
takes 30–60+ min / appears hung. We build each platform on its **native** runner
(arm64 on the free `ubuntu-24.04-arm`, public-repo only), push by digest, then
merge into one multi-arch manifest. Don't collapse this back into a single
QEMU `platforms:` build.

`docker.yml` triggers **only on tags** (not `main`) to avoid a duplicate build
per tagged commit; `latest` is applied per tag.
