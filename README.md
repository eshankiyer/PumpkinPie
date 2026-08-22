<div align="center">

<img src="assets/default_icon.png" alt="" width="96" height="96">

# PumpkinPie

![CI](https://github.com/eshankiyer/PumpkinPie/actions/workflows/rust.yml/badge.svg)
[![License: GPL](https://img.shields.io/badge/License-GPLv3-yellow.svg)](https://opensource.org/licenses/gpl-3-0)

**A Minecraft 26.2 server written in Rust, built for vanilla behavioural parity.**

[Website](https://eshankiyer.github.io/PumpkinPie/) ·
[Download](https://github.com/eshankiyer/PumpkinPie/releases/latest) ·
[Implementation tracker](https://eshankiyer.github.io/PumpkinPie/tracker/)

</div>

## What this is

PumpkinPie is a Minecraft: Java Edition server for 26.2. It began as a fork of
[Pumpkin](https://github.com/Pumpkin-MC/Pumpkin) and has grown into its own project with its own
priority: **matching what vanilla actually does**.

Every behavioural change is checked against a decompiled 26.2 server and cites the file and line
it was checked against. Where it can be, it is also confirmed against a running server with a
real client attached, rather than by reading code alone.

## Parity, measured

Two instruments, deliberately separate, because they answer different questions. Both are in-repo
and reproducible.

### Registry coverage: does an implementation exist at all?

| | covered | total | |
|---|---:|---:|---:|
| Blocks | 990 | 1196 | 82.8% |
| Items | 1420 | 1537 | 92.4% |
| Entities | 158 | 158 | 100% |
| **Total** | **2568** | **2891** | **88.8%** |

That block figure understates things substantially. Auditing all 206 uncovered blocks against
`Blocks.java` found 185 that need no behaviour at all (wool, concrete and dyed terracotta are
literally `(c, p) -> new Block(p)`) and 21 that are data-driven. Exactly one was a real gap.

The item list tells the same story: of 118 uncovered items, 80 were inert crafting materials, 20
were client-only tooltip subclasses, 14 were data-driven, and 4 were real. Counting only
registrations that *should* exist puts both categories near 99%.

### Method-level conformance: how much of each class's surface is covered?

| | |
|---|---:|
| Strict, counterpart in the one mapped file | 21.3% |
| Loose, counterpart anywhere in `crates/` | 50.6% |
| Remaining leads | 4209 |

The truth is between the two. Strict under-credits behaviour split across modules and trait
impls; loose over-credits trait defaults.

A lead is **not** a confirmed gap. Measured yield runs around one in five, the rest being
renames, methods inlined into their callers, or client-only code a dedicated server never
reaches. Audits so far: 1 real gap in 207 block leads, 3 in 14 worldgen leads, 4 in 118 item
leads, 4 in 15 block-entity leads.

Neither instrument is behavioural proof: a matched method can still behave differently, which is
what the ongoing verification work is for.

### Against other Rust Minecraft servers

The same instrument, run unchanged against each project's own checkout. It discovers source
roots per repo, so SteelMC's `steel-core/` layout is measured as fairly as a `crates/` one.

| | PumpkinPie | SteelMC | Pumpkin |
|---|---:|---:|---:|
| Classes with an analogue | **1096** | 894 | 966 |
| Methods matched, absolute | **4303** | 3384 | 3168 |
| Methods scored against | 8512 | 5418 | 7762 |
| Depth per class claimed | 50.6% | **62.5%** | 40.8% |

Read the rows together, because the last one disagrees with the first two and the disagreement
is the interesting part. PumpkinPie covers the most of vanilla and matches the most methods outright.
SteelMC matches a higher *share* of what it takes on, because it takes on less: its percentage is
computed over 5418 methods where PumpkinPie's is over 8512. A project implementing fewer classes
scores higher on a per-class metric while covering less of the game, so neither number alone is
the answer.

Where PumpkinPie is straightforwardly ahead is against the upstream it forked from, on every
measure here.

Two caveats on the comparison. The instrument carries 18 negative-control probes; only 9 apply to
SteelMC, and upstream needed `--no-probes` because one failed against its layout, so cross-repo
confidence is weaker than same-repo confidence. And this is still name-level matching: it does not
prove any of the three behaves correctly.

```sh
python3 tools/tracker/build_surface.py      # registry coverage
python3 tools/conformance/conformance.py    # method-level conformance
tools/run-tests.sh                          # full suite, including a live server

# and against any other server's checkout
PUMPKIN_REPO=/path/to/other-server python3 tools/conformance/conformance.py
```

## Running it

Grab a build from the [releases page](https://github.com/eshankiyer/PumpkinPie/releases), then:

```sh
tar xzf pumpkinpie-x86_64-unknown-linux-gnu.tar.gz
cd pumpkinpie
./pumpkin
```

Binaries are published for Linux x86-64 and ARM64, Windows x86-64 and macOS ARM64, each with a
`SHA256SUMS` file:

```sh
sha256sum -c SHA256SUMS --ignore-missing
```

These binaries are unsigned, so SmartScreen and some antivirus products will warn about them.
That is what an unsigned executable from a small project looks like, not evidence of anything in
particular. Every release links a VirusTotal report so you can read the scan yourself, and
building from source is always an option.

### Configuration

The server writes `pumpkin.toml` on first start. The MOTD accepts legacy formatting codes with
either `&` or `§`: colours `0`-`9` and `a`-`f`, `l` bold, `o` italic, `n` underline, `m`
strikethrough, `k` obfuscated, `r` reset, and `x` for RGB. `\n` starts a second line, and `&&` is
a literal ampersand.

```toml
motd = "&6&lPumpkinPie\n&7Vanilla parity, in Rust"
```

## Testing

`tools/run-tests.sh` runs the whole instrument set in one command: the static instruments, then a
real server on a scratch world with a headless client joining it and a protocol fuzzer against
it. Non-zero exit on any failure.

The headless client, `tools/parity-bot`, earns its keep. It has caught protocol bugs that had
already passed clippy, the unit tests and static review.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). One rule specific to this fork: **any claim about vanilla
behaviour must cite the decompiled source by file and line**, from a read performed for that
change. "Vanilla does X" from memory has been wrong here often enough to be a standing rule.

## Relationship to Pumpkin

PumpkinPie is an independent fork. It is **not** affiliated with, endorsed by, or supported by the
Pumpkin project. Problems seen here belong in
[this repository's issue tracker](https://github.com/eshankiyer/PumpkinPie/issues), not upstream's,
and upstream's Discord is not support for this fork.

Upstream's work is gratefully carried forward, and upstream changes are merged in regularly.

## License & Attribution

- **Server**: [GNU General Public License v3.0](LICENSE), as Pumpkin is.
- **Plugin API** (`pumpkin-plugin-api`, `pumpkin-plugin-wit`): dual-licensed
  [MIT](crates/pumpkin-plugin-api/LICENSE-MIT) OR
  [Apache-2.0](crates/pumpkin-plugin-api/LICENSE-APACHE), for flexibility when writing plugins.
- **Third-party assets & data**: Bedrock mappings, protocol conversion data and Minecraft assets
  are subject to their own licenses and attribution terms. See
  [assets/NOTICE.md](assets/NOTICE.md).

Not affiliated with Mojang Studios or Microsoft. Minecraft is a trademark of Mojang Synergies AB.
