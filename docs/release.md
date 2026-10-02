# Publishing a release

Users install with `herdr plugin install massdo/herdr-marketplace`.
The installer uses a verified prebuilt binary only when the release's
`SOURCE_COMMIT` matches the installed commit. Every merge into `main`,
including a documentation change, must therefore publish a new version.

## Before merging into main

1. Set a new stable version (`MAJOR.MINOR.PATCH`) in `Cargo.toml`,
   `herdr-plugin.toml` and the `herdr-marketplace` entry in `Cargo.lock`.
   It must be newer than the version on `main`, and its `v<version>` tag
   must not exist yet. Development commits on other branches do not each
   need a new version.
2. Open a pull request into `main`. The existing required checks,
   `check (ubuntu-latest)` and `check (macos-latest)`, run
   `scripts/release-version.sh` against the PR's base commit before testing.
   They refuse an unchanged version, a downgrade, mismatched versions or
   a reused tag. Keep both checks required and require the branch to be
   up to date before merging.
3. Merge with **Create a merge commit** (`gh pr merge <PR> --merge`). Keep
   **Require linear history** disabled, administrator enforcement and
   conversation resolution enabled. Do not create the tag manually.

## Automatic publication

Every push to `main` starts the `release` workflow. It validates the three
versions, runs the offline suite on macOS and Linux, and builds the plugin
and its private FFmpeg for Apple Silicon, Intel macOS and Linux x86_64.

After all checks and builds pass, the workflow:

1. Creates `v<version>` on the exact commit that triggered the run. An
   existing tag is accepted only if it points to that same commit.
2. Creates a draft release with the three plugin binaries, three FFmpeg
   binaries, `SHA256SUMS` and `SOURCE_COMMIT`.
3. Downloads the uploaded assets, compares them with the local artifacts,
   checks the tag's commit again, and publishes the release.

Tag creation and publication run in the same workflow: a tag created with
`GITHUB_TOKEN` does not trigger another workflow. If a run fails, fix its
cause and rerun it; it can reuse its tag and resume its draft. A published
release is never overwritten. A code fix requires a new version.

Until publication finishes, installs of the new `main` commit still fall
back to a source build. A failed release must be resolved before treating
the version as delivered.

## Verify installation

Herdr hides successful build output. Ask the script for its log:

```sh
HERDR_MARKETPLACE_BUILD_LOG=/tmp/build.log herdr plugin install massdo/herdr-marketplace --yes
cat /tmp/build.log
```

It should say `installed verified prebuilt v<version>`. README videos use
the verified private FFmpeg installed alongside the plugin binary.
