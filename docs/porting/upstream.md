# Tracking upstream

This repository is a fork of `dylanebert/shallot`. For now the Rust port follows upstream's features; later it may diverge.

## Branches and remotes

- `upstream` remote: `https://github.com/dylanebert/shallot`.
- `main` here carries upstream's history plus our additions (`crates/{platform,gpu,wgsl,formats,engine,core,standard,extras,shallot,abi,cli}`, `examples/*/rs`, `docs/porting`, CI for the port).
- Upstream is merged, not rebased, so the TypeScript tree stays identical to an upstream commit at every merge and the TypeScript pre-land check keeps passing.

## Cadence

Weekly, or when upstream tags a release, whichever comes first:

1. `git fetch upstream && git merge upstream/main` on a branch named `sync/upstream-<date>`.
2. Resolve conflicts. Expected only in the root `Cargo.toml` (workspace members), `crates/physics` (our native changes), and CI files.
3. Run the TypeScript pre-land check (`AGENTS.md`) and the Rust suite.
4. Classify the merged upstream commits with the porting ledger (below).
5. Regenerate the WGSL corpus and the screenshot goldens if shaders or rendering changed; the parity job then shows exactly which Rust modules are behind.

## Porting ledger

`docs/porting/status/upstream-ledger.md` lists every upstream commit since the snapshot in [README.md](README.md) with one of:

| State | Meaning |
|---|---|
| `ported` | The Rust side has the equivalent change; link to the commit |
| `pending` | Needs porting; linked to an issue |
| `n/a` | Touches only TypeScript tooling, browser-only behaviour, or files with no Rust counterpart (`src/native`, Vite, publish) |
| `shared` | Changed shared code (`crates/physics`, `crates/audio`) that both builds consume; ported by the merge itself |

The [module map](module-map.md) tells the reviewer which Rust module each TypeScript path maps to, so classification is mechanical. A script lists the upstream commits not yet in the ledger and the Rust modules their paths map to.

## Keeping `crates/physics` mergeable

Upstream edits the physics kernel often. Our changes there are kept to the seams listed in [physics-audio.md](physics-audio.md#changes-to-cratesphysics): `cfg` gates, the native thread wait, and a new `native.rs`. They are small, isolated, and good candidates to offer upstream as a pull request, which would remove the conflict surface entirely.

## When to diverge

Divergence is Matt's call. Signs it is due: upstream changes that make no sense natively (browser-only features), or Rust-side improvements that upstream will not take. When it happens, record it in [decisions.md](decisions.md), freeze the ledger, and keep the TypeScript tree as a historical reference.
