# Packaged Bun consumer

Run from the repository root on Linux with Bun 1.3.13 and the Rust development shell:

```sh
cargo build --locked --features host --bin agent-bridge-host
cd host
bun install --frozen-lockfile
cd ..
bash verification/consumer/verify.sh
```

The check packs the private TypeScript package, creates a fresh directory under
`/tmp`, and installs the tarball there. It copies the built host and existing test
provider into that directory. The consumer imports only the package's declared
entry points. The repository supplies the TypeScript compiler and ambient Bun
types for checking; the installed package has no runtime dependencies.

The consumer discovers a configuration catalog and switches models between two
turns using the compiled configuration fixture. It then starts a tool session,
delivers selected text context, validates the returned JSON, reads streamed text, handles an application tool
and a scripted question, cancels a second invocation, and closes the host. A new
host then reads the exact saved records, builds a typed transcript projection,
discovers saved sessions/runs/continuations, and explicitly resumes a native handoff.
Assertions check a fresh application token, the persisted answer, input/result and tool receipts,
handler cancellation, and terminal outcomes. Execution has a 60-second deadline.

CI runs this same check. The printed temporary directory retains the installation,
tarball, binary, and database for inspection; each invocation creates a new directory.
No provider account, registry publication, or binary download is required.

This establishes an isolated package-consumer check using a deterministic provider.
Live OpenCode/Codex verification, Rust crate packaging, database upgrade artifacts,
and independent developer review remain separate release checks. The client is
Bun-specific and the host path remains explicit.
