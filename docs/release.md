# Public alpha release checklist

H4 is in progress. The artifacts are locally installable, but this is not a public
release. The Rust candidate is `agent-bridge-acp 0.1.0-alpha.0`, with publication
allowed only to crates.io. It has not been uploaded. The Bun package remains private
under its existing `agent-bridge` name and `0.1.0-alpha.0` version.
The Rust library import stays `agent_bridge`; the executable stays `agent-bridge-host`.

## Verified locally

On September 12, 2026, the [consumer script](../verification/consumer/README.md)
installed the release-profile Rust host from an extracted Cargo archive and installed the Bun
tarball outside the checkout. All packaged Rust targets compiled. The separate
Rust integration application and the combined Bun workflow passed. A frozen
schema-1 database upgraded to schema 7 without changing its record evidence or
application data. Unsupported schema and wire versions were rejected.

All 56 host tests passed against the installed host, including Linux process-tree
cleanup, stalled pipes, cancellation, and crash discovery. Linux is the only
advertised host platform for this alpha. Bun 1.3.13 is the exercised client runtime;
Node compatibility and Windows/macOS lifecycle guarantees are not claimed.

The build reuses local Cargo caches and development tooling. Registry installation,
binary distribution and a remote CI result have not
been established by that check. Source installation is the current delivery path.
No prebuilt binary download service is required for this alpha.

MIT licensing is selected; both package manifests declare it and both packages
include the license text.

## Before publication

Candidate preparation passed 193 Rust tests, formatting and Clippy, plus the
packaged-consumer workflow and 56 host tests against the installed release binary.
Cargo's online `publish --dry-run --locked --all-features` also passed without an
upload. Chesscave compiled with the renamed package alias while retaining its
local path. These are local checks, not a remote CI result or independent review.

- Confirm availability of the selected Rust name and verify the exact candidate
  with the consumer script and `cargo publish --dry-run --locked --all-features`.
  The initial registry API lookup returned HTTP 403, so name availability is unconfirmed.
- Authenticate locally for crates.io publication. Never put a registry token in
  source control or chat. No actual publication is part of the preparation checks.
- Independent developer review is deferred by the maintainer for this experimental
  Rust alpha. It is not a publication prerequisite for this release and has not been
  completed. The repository's own consumer and Chesscave trial are not independent review.
- Record the final artifact checks and actual CI result, then publish the packages,
  tag the release, and provide installation instructions with these support limits.

Provider compatibility is version-specific. See [host verification](../host/verification.md)
for dated real-provider results. Authenticated Claude verification remains deferred.
The alpha does not promise automatic effect reconciliation, native-state inspection,
power-loss durability, or safe concurrent recovery by multiple host processes.

## Reviewer handoff

Give the reviewer both archives, the source-install command from the consumer
script, and the [integration guide](integration.md). Ask them to use their own
application outside this checkout, run a tool/question workflow, cancel an active
run, restart and inspect saved history, and report the exact OS, Bun, Rust, and
provider versions. Record their findings and resolutions here; no review has yet
been recorded.
