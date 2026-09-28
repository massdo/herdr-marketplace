# Publishing a release

`herdr plugin install` clones the repository, then runs
`scripts/fetch-or-build.sh`. The script installs the binary of release
`v<version>`, where the version comes from `Cargo.toml`, only when the
release's `SOURCE_COMMIT` is the commit being installed. Any other commit is
built from source. A commit of `main` without its own release therefore makes
every new install compile: tag each version as soon as it reaches `main`.

## Steps

1. On `staging`, set the new version in `Cargo.toml` and `herdr-plugin.toml`,
   then run `cargo build` so that `Cargo.lock` follows.
2. Merge `staging` into `main`. `main` only accepts a linear history: use
   the rebase merge.
3. Tag the commit `main` points to, and push the tag:

   ```sh
   git fetch origin
   git tag -a v0.1.0 -m v0.1.0 origin/main
   git push origin v0.1.0
   ```

4. The `release` workflow refuses a tag that does not match the three
   versions, runs the offline suite on macOS and Linux, builds the three
   binaries, then publishes `herdr-marketplace-aarch64-apple-darwin`,
   `herdr-marketplace-x86_64-apple-darwin`,
   `herdr-marketplace-x86_64-unknown-linux-musl`, `SHA256SUMS` and
   `SOURCE_COMMIT`. A failed run can be run again: it resumes the draft and
   never overwrites a published release.
5. Check an install. Herdr hides the output of a build that succeeds, so ask
   the script for its log:

   ```sh
   HERDR_MARKETPLACE_BUILD_LOG=/tmp/build.log herdr plugin install massdo/herdr-marketplace --yes
   cat /tmp/build.log
   ```

   It should say `installed verified prebuilt v<version>`.

A version is published once: to fix a release, publish the next version.
