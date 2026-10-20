# DECISIONS.md

Decisions taken autonomously during the parity campaign. Never re-litigate a row here
without new evidence.

---

## 2026-07-27 - Process supervision uses setsid, not tmux

The mandate says long-running processes go under tmux. tmux, screen, dtach and abduco
are all absent from this machine and `sudo -n` fails (password required), so no package
can be installed, and pulling a third-party static tmux build is out of policy.

`setsid bash -c "..." </dev/null >log 2>&1 &` was already proven in this project to
survive the MCP shell exiting, which is the property tmux was wanted for. All
long-running work uses that idiom. If tmux ever becomes installable the launcher can be
swapped with no other change.

## 2026-07-27 - `pkill -f ollama` is banned in this project

`pkill -f <pattern>` matches the `bash -c "..."` wrapper running the pkill itself
whenever the pattern appears in the command string, so the shell kills its own ancestor
and every later command silently never runs. This produced an "empty output, VRAM still
pinned" failure that looked like the kill had no effect.

Always resolve PIDs first (`ps -eo pid,cmd | grep ...`) and `kill -9 <pid>`.

## 2026-07-27 - Model upgrade: qwen2.5-coder:7b -> Qwen3-Coder-30B-A3B-Instruct UD-Q4_K_XL

unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF, file
Qwen3-Coder-30B-A3B-Instruct-UD-Q4_K_XL.gguf, 17.67 GB, in ~/pumpkin-local/gguf/.

30B total / 3B active MoE. Full weights do not fit in 8 GB of VRAM, so MoE expert
tensors are pinned to system RAM with -ncmoe while attention, embeddings and the KV
cache stay on the GPU. Active-parameter count drives token latency, so a 3B-active
model reading experts from RAM should beat a 7B dense model that fits entirely in VRAM,
and it is a much stronger coder.

Rejected: Q4_K_M (18.56 GB, not a dynamic quant), UD-Q5_K_XL (21.74 GB, KV cache would
not fit alongside), UD-Q3_K_XL (13.81 GB, quality drop unnecessary since the experts
live in RAM either way).

## 2026-07-27 - Every llama-server launch re-checks VRAM

On an 8 GB card one orphaned llama-server breaks the next run: it pins several GB and
llama.cpp silently falls back to "offloaded 0/48 layers to GPU", which reads as a slow
model rather than a leaked process. Observed twice, most recently 5070 MiB held by an
orphan whose parent `ollama serve` was already dead.

~/pumpkin-local/serve.sh kills strays by resolved PID and asserts free VRAM above a
threshold on every invocation, not once at setup, and refuses to start if the card is
not clear.

## 2026-07-27 - Serving stack: raw llama-server, Ollama retired

The 30B GGUF is used directly with the llama-server binary that ships inside Ollama
(`~/.local/ollama/lib/ollama/llama-server`, build b4d6c7d8f), which supports `-ncmoe`,
`-ot`, and `-fa`. Ollama's own model store cannot express per-tensor CPU offload, which
is the entire point of this configuration, so `ollama serve` is no longer used.

The harness talks OpenAI-compatible HTTP to llama-server on port 11434, the same port
Ollama used, so the spec runner needs no endpoint change.

## 2026-07-27 - RETRACTION: the "game rule default" divergences were my RCON client

The oracle's RCON client read exactly one packet per command. Pumpkin splits one
command response across several RCON packets; vanilla's fit in one. So on Pumpkin
the first fragment was returned and the rest stayed queued, and every subsequent
command read a stale fragment. The whole session slid one reply out of step.

Because only Pumpkin fragmented, only Pumpkin's answers looked wrong, which is
indistinguishable from a parity bug at the measurement layer. It produced twelve
confident-looking game-rule divergences: command_block_output "UNSUPPORTED",
fall_damage "UNSUPPORTED", forgive_dead_players "false", and so on.

All twelve are false. After fixing the client to drain every fragment carrying the
request id, `gamerule_defaults` matches vanilla exactly across all forty rules.

Do not act on any measurement taken through the old client. What survives
re-measurement is `command_surface`: debug, function, jfr, locate, place, return,
test and waypoint are genuinely missing from Pumpkin.

This is the second time a measurement artifact has been mistaken for a Pumpkin bug
(the first was file-descriptor exhaustion). Both times the tell was the same: a
suspiciously large batch of divergences appearing at once in an area nobody had
touched. Treat that shape as an instrument fault until proven otherwise.

## 2026-07-27 - Probe gate is the Phase 2 acceptance test

~/pumpkin-local/oracle/probe_gate.py. `--write-baseline` records every probe's
status; `--gate NAME` passes only if NAME went divergent -> matching and no other
probe went matching -> divergent. Exit 4 means a server was unhealthy, which is
deliberately NOT a divergence verdict: an unmeasurable server must abstain rather
than send the agent to fix code that was never wrong.

## 2026-07-27 - Phase 1 complete: 9.56 tok/s, gate was 8

Qwen3-Coder-30B-A3B-Instruct UD-Q4_K_XL, llama-server, ctx 32768, flash attention
on, `-ngl 99 -ncmoe 42`, 6479 MiB of 8151 MiB resident.

Benchmark is a real spec, not a synthetic prompt: specs/07-weighted-plate.md with
the full system prompt and the target file attached, 2288 prompt tokens.

    ncmoe=48 (all experts in RAM)   4415 MiB   9.17 tok/s
    ncmoe=42                        6479 MiB   9.56 tok/s
    ncmoe=34                        failed to create context (KV cache did not fit)

42 is the setting. The first reply was a correct unified diff on attempt one,
which the 7B never managed on this spec in diff mode.

The tok/s spread between 48 and 42 is small because the bottleneck is expert
weights streaming from system RAM either way. Do not chase it further by lowering
ncmoe: 34 already fails, and the failure is a hard context-creation error at
startup, not a graceful degradation.

## 2026-07-27 - The CUDA backend is a dynamically-loaded plugin and needs an exact path

llama-server bundled with Ollama does not link CUDA. The backend is
`lib/ollama/cuda_v13/libggml-cuda.so`, loaded at runtime.

`GGML_BACKEND_PATH` must name the .so itself. Pointed at the containing directory
it logs "cannot read file data: Is a directory", then continues and reports "no
usable GPU found, --gpu-layers option will be ignored". The server starts, serves
correctly, and runs a 30B model entirely on the CPU. The only symptom is that it
is slow, which is indistinguishable from a bad quantisation choice.

serve.sh now exports the full .so path plus LD_LIBRARY_PATH for libcublas, and
hard-aborts if `--list-devices` does not report a CUDA device, rather than
silently falling back to CPU.

## 2026-07-27 - Ollama is retired

`ollama serve` and the 7B (qwen2.5-coder:7b) are no longer used by anything. The
harness, run_all and driver all speak OpenAI-shaped HTTP to llama-server on 11434.

## 2026-07-27 - PARITY_SCORE baseline: 51.45 / 100

First measurement of the objective function. Components: command surface 28.95 of
50 (11 of 19 sampled vanilla commands present), probe agreement 22.50 of 30 (3 of
4 probes matching), backlog burndown 0.00 of 20 (0 of 43 confirmed rows marked
Fixed).

Missing commands, all confirmed against vanilla 26.2 over RCON: debug, function,
jfr, locate, place, return, test, waypoint.

The backlog component is weighted lowest deliberately. It is the only one that
moves by editing a markdown file, and an objective function you can satisfy by
editing the scoreboard is not one.

## 2026-07-27 - Phase 5 scaffolding: /locate

Converted the unbuilt /locate implementation into skeleton plus tests plus specs
rather than finishing it by hand. Three pure helpers stubbed with todo!(), twelve
unit tests that define their behaviour, and specs 20 to 22 pointing at them.

Two things the workspace lint config forced:

- `clippy::todo` is denied, so a todo!() skeleton does not compile under the
  gate. Each stub carries `#[expect(clippy::todo, unused_variables, reason=...)]`
  and the spec instructs the model to delete the attribute when filling it.
- `dead_code` had to be `allow` rather than `expect`. In the test build the
  functions ARE used, so an `expect` goes unfulfilled, and
  `unfulfilled_lint_expectations` is itself denied. An expectation that is only
  sometimes correct fails the build in the configuration where it is wrong.

The executor is deliberately not stubbed out to the model. It needs a live World,
a generator that may be Flat and therefore have no structure cache, and the world
seed, none of which a unit test can supply, so handing it over would be handing
over a task whose gate cannot see it.

Structure tags are not supported and the code says so: `pumpkin-data` has no
`worldgen/structure` tag registry, so `#minecraft:village` has nothing to resolve
against. Reporting it invalid beats silently matching nothing.

## 2026-07-27 - Escalation tier wired, initially blocked, now live

`escalate.py`: specs the local 30B could not finish get exactly one attempt from
the Claude Code CLI on sonnet, hardest-first by oracle value, three per night.

The quota probe runs BEFORE each escalation and any rate-limit signal ends the
night's escalation rather than triggering a retry. A retry loop against a limit
burns the quota the next morning's session needs and turns a temporary condition
into an outage. A failed escalation goes to PENDING_SESSION.md and is never
retried automatically: a spec that failed both tiers is failing for a reason a
third identical prompt will not fix.

At first wiring the CLI reported "Not logged in" and the tier self-disabled,
which is the correct behaviour and was logged as blocked rather than worked
around. After login the probe returns available.

## 2026-07-27 - Phase 4 shipping rules

`ship.py`. Local remotes (forgejo and fork) get every branch that passes the
gate, immediately. Upstream is gated on the divergence reproducing in at least
two separate oracle rounds, capped at five per day, with the excess queued in
PENDING_UPSTREAM.md rather than dropped.

The two-round rule is not caution for its own sake. Twice on this project a batch
of confident-looking divergences turned out to be the measurement layer failing:
file-descriptor exhaustion once, an RCON client reading one packet per command
the other time. A single round is a reading. Two independent rounds agreeing is
what makes a maintainer's time safe to spend.

Pipeline commits use a distinct author, "Pumpkin Parity Pipeline"
<rdravida+pumpkin-pipeline@gmail.com>, set per-invocation with `git -c` so no
global config is touched. Pushes are never forced and never to main.

## 2026-07-27 - Model candidate: Qwen3.6-35B-A3B, and why not NVFP4

Evaluating `unsloth/Qwen3.6-35B-A3B-GGUF` at `UD-IQ4_XS`, 17.73 GB, against the
incumbent Qwen3-Coder-30B-A3B UD-Q4_K_XL at 17.67 GB. Same 3B active parameters,
so the same speed class, and near-identical memory footprint.

Published numbers for 3.6: SWE-bench Verified 73.4, SWE-bench Multilingual 67.2,
Terminal-Bench 2.0 51.5, LiveCodeBench v6 80.4. Native context 262,144.

NVFP4 was requested and is not usable here. `unsloth/Qwen3.6-35B-A3B-NVFP4`
exists, but NVFP4 is a TensorRT and vLLM format, not a llama.cpp one, and it only
pays off with the weights resident on the GPU. This configuration is the exact
opposite: MoE experts live in system RAM via `-ncmoe` and only attention plus the
KV cache stays on the 8 GB card. vLLM has no equivalent of per-tensor CPU expert
offload, and 17.5 GB of FP4 weights do not fit in 8 GB either way.

Q4_K_XL of this model is 22.36 GB and does not fit in 30 GB of system RAM
alongside everything else. IQ4_XS is the largest quant that keeps the current
memory profile.

## 2026-07-27 - Reasoning is bounded, not disabled

Qwen3.6 thinks by default. First instinct was to switch it off entirely, because
GLM-4.7 destroyed an earlier version of this pipeline by emitting 20k tokens of
chain of thought for a task whose answer was fifty tokens. At ~9 tok/s an
unbounded preamble is minutes per attempt against a 6-iteration cap.

Off is the wrong answer though: it discards what this model is actually good at.
The server now runs `--reasoning on --reasoning-budget 512 --reasoning-format
deepseek`. Think briefly, then answer. When the budget is exhausted llama.cpp
injects the end-of-thinking tag and the model proceeds, so a truncated thought
degrades into a fast answer rather than a dropped response.

`--reasoning-format deepseek` matters as much as the budget: it routes the
thought into `message.reasoning_content` so it never reaches the code parser. A
`<think>` block left in `message.content` would be written verbatim into a Rust
file and fail to compile with an error pointing somewhere else entirely.

`strip_thinking` in func_edit.py is defence in depth for that case, including the
unclosed-tag form produced when the budget cuts a thought off mid-sentence. It
should never fire. If it does, the alternative was silent source corruption.

## 2026-07-27 - Qwen3.6-35B-A3B adopted: 22.37 tok/s, 2.3x the incumbent

Measured on real specs, not synthetic prompts.

    model                          ctx     ncmoe   VRAM       tok/s
    Qwen3-Coder-30B UD-Q4_K_XL     32768   42      6501 MiB    9.56
    Qwen3.6-35B UD-IQ4_XS          16384   42      2991 MiB   13.37
    Qwen3.6-35B UD-IQ4_XS          16384   34      5275 MiB   22.37
    Qwen3.6-35B UD-IQ4_XS          16384   26      failed to create context

The context reduction is where most of the win came from, and it was not the
obvious lever. Halving the KV cache freed 3.5 GB of VRAM, which paid for eight
more expert layers on the card, and expert placement is what actually drives
throughput here. 32768 was never needed: FUNC mode sends one function, and the
largest ever fed to the model was 111 lines.

Both answers in the benchmark were correct on the first attempt, including the
f64-before-arithmetic requirement in spec 22 that exists specifically to avoid an
i32 overflow at world edge.

Bounded reasoning works as intended: roughly 1900 characters of thought landed in
`message.reasoning_content` and never touched `message.content`.

## 2026-07-27 - The machine was swapping, and that was the real failure cause

First full queue run: 4 candidates, 13 max-iteration failures. The failures
looked like model quality. They were not.

`llama-server` had 8.2 GB swapped out. 30 GB of RAM against 17.6 GB in use, a
16.8 GB model mapped, plus Steam, Discord and the desktop. Every token was
touching disk.

The context reduction above cuts the resident footprint as well as raising
throughput, which addresses this from both directions. If failures persist, the
next lever is closing the desktop applications rather than blaming the model, and
the one after that is a smaller quant. Do not conclude "the model is weak" from a
run taken while the machine is swapping. That is the same class of mistake as the
file-descriptor and RCON-desync incidents: an environment fault wearing the
costume of a code fault.

## 2026-07-27 - Specs may share a worktree, and the runner must cope

The three /locate stubs live in one file, so specs 20, 21 and 22 share a
worktree. The lock serialised them correctly, but each one left its edit behind,
and the next spec saw a dirty tree and refused with DIRTY. 21 and 22 therefore
never ran at all, which read as "the agent stopped early".

The runner now resets a dirty worktree instead of refusing, but only once some
spec in this run has recorded a result. If nothing has run yet, the dirt belongs
to a human and is not ours to discard. Any diff worth keeping is written to
/tmp/pumpkin_<name>.diff before the worktree is released, so the reset loses
nothing.

## 2026-07-27 - Spec 06 pointed at the wrong function

The model refused it with "This function does not contain the critical hit logic
... `handle_knockback` is unrelated to the critical hit conditions described".
It was right. The spec's FUNC line named `handle_knockback`; the crit conditions
live in `AttackType::new` in the same file. Retargeted.

Worth recording because the failure mode is instructive: a well-written CANNOT is
the model working correctly, and burning six iterations against a mis-aimed spec
is a spec bug that shows up in the statistics as a model failure.

## 2026-07-28 - The retry loop contradicted FUNC mode, so no spec could recover

Post-mortem of the overnight run found the dominant failure was neither the
model nor the specs: the retry messages after a failed build demanded
SEARCH/REPLACE blocks or unified diffs, while FUNC mode accepts only a
complete function starting at the signature line. One failed build therefore
made the rest of the run unwinnable. The model obeyed the retry format, the
applier refused it, and the complaint pushed it further away. Spec 21 shows
the whole arc: apply, build fail, four format rejections, spontaneous recovery
to function form at iter 6, death on the attribute lint.

Three changes follow. Retries are now mode-aware (format_reminder in
run_all.py) and re-show the current code only when an edit actually landed.
Line-edit mode adopts the written line as its new target, because specs 01 and
02 spent five iterations searching for a line their own first edit had
replaced. And the error text sent to the model is persisted to
/tmp/pumpkin_<name>_errors.log with the first line echoed into the run log:
three rounds died on one 288-character lint that nobody ever read.

Specs 05 and 18 are retired to specs/blocked/ with reasons in their headers:
05 asserts the damage type is reachable from arguments that do not carry it
(the model CANNOT was correct, again), and 18 is a multi-file change wedged
into a diff applier that has never landed an edit. Neither failure says
anything about Qwen3.6. The queue results so far measure the harness, the
specs, and the machine, in that order, and the model last.

## 2026-07-28 - Be the autocomplete: generated API surfaces kill invented methods

The dominant model failure once retries worked was invented APIs: is_in_water
on Player, is_raining_at_unchecked on World, BlockState::from_nbt, light
methods on BlockAccessor. A model coding against types it cannot see guesses
method names the way a human would without autocomplete, because that is what
it is doing.

Four pieces, all generated from the worktree so none can go stale. apisurface.py
extracts real impl/trait signatures for the types a prompt mentions; the FUNC
first prompt now carries that pack (spec 14 measures ~1000 tokens for the four
types in play). FUNC_SYSTEM gained a second rule: project-type methods not
visible in the shown code, the listing, or a compiler suggestion do not exist,
CANNOT instead of guessing. E0599 retries inject the real surface of exactly
the type the compiler named. And specs may promise APIs with API: lines, which
lint_specs verifies against the tree, because a false promise is worse than
none: the model trusts it over its own caution.

Not covered, deliberately: wrong semantics of real APIs (build, tests and the
probe gate own that) and E0425 scope errors (partially helped at best).


## 2026-07-28 - Post-mortem of the six failures, and the rerun kit

The six failures of the first fixed-harness run split cleanly. 06 and 12:
invented APIs, the class the surface pack now covers, failed before it landed.
09: a FUNC blind spot, the model made a pre-existing use line unused and could
not delete a line outside its span; fix_unused_imports now does it
deterministically from rustc file:line, including single members of group
imports, with one rebuild when imports were the only errors. 01 and 10: real
model weaknesses, integer-cast discipline and pattern-vs-expression rules,
repeated across iterations without learning. 02: reply truncated mid-word;
cause was the interaction of the 512 reasoning budget with the 3000 max_tokens
TOTAL decode cap.

Reasoning is now uncapped (serve.sh default -1) and max_tokens is 6000, so a
long thought cannot push the code out of the window. Iterations will be
slower; that is the correct trade for the hard specs. Every iteration's reply
is kept at /tmp/pumpkin_<name>_r<iter>.txt.

rerun.sh reruns whatever the status file says failed, refusing while a queue
run is live. --coder swaps the server to Qwen3-Coder-30B-A3B for the residue.
The model-swap question stays open until a rerun with all of today's fixes
separates our failures from the model's: as of tonight, every investigated
failure but 01 and 10 was ours.


## 2026-07-28 - A triage judge, not a second coder

The queue's biggest waste was iterations spent on structurally unwinnable
situations: six retries against an error the span cannot reach, or a premise
the source contradicts. So the first agent added to this pipeline judges
instead of writes. After 2 and 4 failed iterations, one model call over the
in-memory failure history returns RETRY, BLINDSPOT, BADSPEC, or ESCALATE.
Non-RETRY verdicts stop the spec, go to TRIAGE.md for review, and surface in
the results table. In-memory history only: the /tmp error logs span runs and
would contaminate the evidence.

Safety properties, in order of importance: the judge never touches code; any
failure of its own (timeout, empty reply, unparseable output) is RETRY, so a
broken judge cannot kill a healthy spec; and a wrong stop is recoverable
because rerun.sh re-queues everything that is not a candidate. First live
call found a real bug in its own plumbing: max_tokens=400 was consumed
entirely by thinking, content came back empty, and the RETRY default absorbed
it exactly as designed. Now 2000. Second call returned a coherent RETRY with
evidence.

Generative components sit at decision points; the apply path stays
deterministic. Today proved that ordering everywhere it was tested.


## 2026-07-28 - The loop gets a score, which makes it a loop worth running

Audited against the "Karpathy loop" as the term is used now (propose, run,
score, keep best, discard regressions, iterate; the scoring function is the
load-bearing part). The harness had propose/run/iterate and exit gates but no
score: a worse iteration silently became the new starting point. Spec 10 went
from 18 errors to 36 and the loop kept digging from 36.

Now every failed iteration is scored (stage rank: build < asserts < tests,
then fewer rustc errors within a stage), the best version of the spec's files
is snapshotted, and a regression is rolled back before the retry is built, so
the model is told explicitly that it is improving on its best attempt, not its
last one. TARGET mode's tracked line follows the rollback, or the next apply
would aim at a line the rollback removed.

Also: Qwen3.6-27B dense (the April release, not the NVFP4 build; 19.7 GB of
NVFP4 does not fit a 12 GB card and vLLM has no offload story for it) is
downloading as UD-Q4_K_XL GGUF. serve.sh gained LLAMA_NGL for partial offload
and rerun.sh gained --dense27, marked untuned. Expect single-digit tok/s:
dense pays all 27B every token, which is the price of leaving A3B sparsity.
Bench with capprobe.sh before trusting it with the queue.


## 2026-07-28 - Dense on the 8 GB card, and spec 12 becomes three specs

Corrections first: this card is 8 GB (8151 MiB), not 12. Earlier notes saying
12 GB were wrong. Dense partial offload therefore needs llama.cpp auto-fit,
and auto-fit refuses if ANY placement flag is user-set: pinning -ngl 28
aborted, and even passing -ncmoe (meaningless for dense) counted as a manual
override and forced a full-GPU load that OOMed. serve.sh now omits both flags
when LLAMA_NGL=-1 / LLAMA_NCMOE=0, which is what rerun.sh --dense27 sets.
Result: Qwen3.6-27B loads with auto placement. Throughput still unmeasured.

Spec 12, triage-confirmed BLINDSPOT (three functions, FUNC returns one), is
retired to blocked/ and replaced by 23/24/25: write_nbt, chunk_data_nbt,
from_nbt as independent single-function specs on the shared piston worktree,
locate-style. Every API each spec names was verified in-tree first
(from_id_with_block, properties/to_props/from_props/to_state_id,
put_compound/get_compound, from_registry_key; the read recipe mirrors
BlockStateResolver::resolve, which parses the same Name/Properties shape).
All three lint clean, including the API: promise checks. The nightly's
specs/*.md glob picks them up automatically.

The rerun with today's full machinery banked five new candidates (06, 17, and
the locate trio), nine total, and the triager's first production verdicts were
correct on all four non-RETRY calls.


## 2026-07-29 - One model on disk, and it is the dense one

The A3B ggufs (Qwen3.6-35B-A3B, Qwen3-Coder-30B-A3B) are deleted at Ragini's
request; Qwen3.6-27B dense is the only local model. serve.sh defaults repoint
to it with auto-fit placement (NGL=-1, NCMOE=0), rerun.sh's model flags are
accepted and ignored, and run_all's model label follows. Nothing that runs
tonight references a deleted file.

Measured on the 8 GB card with auto-fit: 2.8 tok/s decode, first reply 242s.
That is 9x slower than the deleted 35B-A3B. The trade is deliberate and the
user's: one stronger, slower brain over a faster, weaker one. The escalation
ladder now has no local second tier; escalation means API or nothing.


## 2026-07-29 - grug-27b queued for an A/B against the base 27B

Ragini proposed two swaps today. The DavidAU "Fable-Fusion-711" merge was
declined with reasons (abliterated RP merge, incoherent benchmark claims, and
a 30 GB Q8 that recreates the documented swap incident). grug-27b is the
opposite case and is downloading: a LoRA on the exact base we run, targeting
the measured bottleneck. Our replies cost 30+ minutes at 2.8 tok/s because
uncapped thinking runs thousands of tokens, and one reply came back empty
when the thought ate the whole decode budget. grug trains 10-198x shorter
thinking at benchmark parity (HumanEval 87.2 vs 87.8, MBPP above base), with
agent-loop hygiene (stuck-loop escape) baked in. Its card publishes results
against itself, including the cell it loses; that is what honest
self-reporting looks like. Still self-reported, still a week old: hence an
A/B, not a swap. When the current batch finishes, run the residue under
grug-27b-qat-Q4_K_M and let scores and candidates decide.


## 2026-07-29 - The reboot that audited our storage choices

Something took the machine down mid-run (dense run and llama-server died
without a summary; /tmp wiped for the second time today). /tmp held the
candidate diffs. Seven were rescued from their still-dirty worktrees
(04, 06, 07, 08, 09, 17, and one of the locate trio); 03 and two locate
diffs were lost and re-queued to be re-earned. Candidates now write to
~/pumpkin-local/candidates/, because work product does not belong in a
directory the OS is allowed to erase. Diagnostics (replies, error logs,
triage transcripts) stay in /tmp on purpose: losing those costs nothing.

The A/B chain also misfired: its wait pattern matched "python3 run_all"
adjacency that the real command line (python3 /full/path/run_all.py) never
had, so it fired early and rerun.sh's guard absorbed it. Armed chains keep
proving fragile; the guard keeps proving essential.

grug-27b is now live on 14 specs: the 8 residue, the 3 re-earnable, and the
3 new piston specs on their first outing. First decode at 3.37 tok/s. The
A/B question: candidates per hour versus the base 27B, whose data point is
roughly one candidate (09) in five hours.


## 2026-07-29 - MULTI mode: bounded multi-function edits, and repo context

The triager's dominant verdict is BLINDSPOT: fixes that span more than one
function. MULTI mode is the bounded answer. A spec with several FUNC lines
shows the model every named function, each returned whole in its own fence,
each applied by the same deterministic span replace as single mode, routed by
signature so order and subset are both fine. It deliberately does NOT reopen
free-form diffing, which failed 9/10 on bookkeeping and is the reason FUNC
mode exists at all. All prompts also gained a repo_map: the module tree of the
touched crates, edit's own directory first, so the model references real paths
instead of inventing modules (spec 15 hallucinated `pumpkin_api`).

Honest finding while wiring this: the current live blindspots are NOT actually
multi-existing-function cases. 15 and 16 need a NEW item (a trait member, a
durability helper) - an INSERT, which MULTI cannot do because there is no
existing span to replace. 21 is an API-visibility miss fixable with API:
lines, not MULTI. So MULTI clears the piston-class (already split into 23/24/25)
and any future "change these N functions together" spec, but the next real
lever for the blockers on the board is an INSERT capability: add a named item
into a named module or impl block. That is the honest next task, not more
MULTI specs. Left unbuilt tonight rather than half-built.


## 2026-07-29 - mini-swe-agent becomes the edit engine; the spree is guardrailed

After surveying the field (OpenHands: needs Docker + 1 GB free RAM, dead on
arrival here; Aider: whole-file edits, too heavy for a 3 tok/s model; opencode:
a product to configure, not hack) the pick is mini-swe-agent: 100 lines,
bash-only, LiteLLM so it talks to llama-server, SWE-bench-credible. Smoke test
passed - it reasoned to `sed -i '85s/0..=10/0..10/'` and the edit landed on the
local 27B. Config gotchas recorded: pre-seed ~/.config/mini-swe-agent/.env
(MSWEA_CONFIGURED=true to skip the wizard, MSWEA_COST_TRACKING=ignore_errors
for the unknown-cost local model).

mini_spree.py is the autonomous loop the bespoke appliers were building toward,
minus the appliers: pull a CONFIRMED gap from WORKQUEUE.md, run mini in the
~/pumpkin-wt/spree worktree, clippy-gate, bank the diff or log the failure.
The edit mechanics that ate two days (FUNC/MULTI/INSERT, retry formats,
unused-import deletion) are now mini's problem, done in bash.

The guardrail is the point, not a footnote. eligible() implements the
WORKQUEUE cardinal rule in code: only STATE confirmed (prefix, so "confirmed,
Wave 2" counts) and vanilla NOT UNVERIFIED are ever implemented. Of 75 items,
12 pass; 63 are held back. "Auto-expand the parity list until full vanilla"
was declined as stated: a 3 tok/s model inventing vanilla behaviour unattended
is exactly the mistake that forced two retractions. Real discovery needs the
differential oracle (vanilla server beside Pumpkin on the NUC) - NUC not
reachable from this session, so that stays the next phase, honestly unbuilt
rather than faked.


## 2026-07-29 - Correction: the NUC was always reachable, oracle already live

I twice claimed the NUC was unreachable and the differential oracle was a
blocked next phase. Both wrong, and worth recording as a process failure: I
tried `ssh nuc` (no such alias) and `eshanki@` (wrong user), got denied, and
concluded "no access" instead of investigating. The user pushed. The username
is `eshan`; the key was authorized all along. Lesson, same class as the
swap/RCON incidents already in this file: a config gap wearing the costume of
a capability gap. Look before concluding absence.

The NUC (eshan@192.168.1.93, now aliased `nuc` in ~/.ssh/config) is a running
homelab. For this project it already hosts the oracle: a `vanilla-oracle`
container (real vanilla MC, RCON 25576, pw oracletest) beside `pumpkin-live`
(RCON 25575, pw pumpkintest), both up for days. nuc_probe.py is a dependency-
free RCON client that sends one command to both and diffs. Live-proven: seed
and difficulty match, time/gamerule differ.

Why this matters more than the fix-spree: the oracle lets parity DISCOVERY
observe vanilla instead of guessing it, which is the only safe way to
auto-expand the list. An RCON diff is real evidence, not a hunch. Caveat kept
honest: not every diff is a bug (error-message formatting, command-syntax
noise), so discovery still proposes suspected-with-evidence for judgement
rather than auto-confirming. But the evidence is now observed, which is the
whole game. Next lever: curated per-behaviour probe batteries (task 22).


## 2026-07-29 - Oracle wired into the spree as a behavioural gate

verify.py connects the oracle to the fix loop. Two entry points: verify_local
(option 2, builds+boots the patched server here on port 25599, probes vs the
NUC vanilla oracle - per-fix, no NUC rebuild) and verify_nuc/deploy_nuc
(option 1, probes the live NUC pumpkin, optionally after a deploy_live.sh
rebuild - slow, for final confirmation). Both proven live: verify_nuc against
the running servers matched on seed/difficulty and correctly flagged a /list
max-players diff.

mini_spree now runs the gate: after clippy passes, an item carrying PROBE
commands gets verify_local, and only banks as VERIFIED if the patched server
agrees with vanilla; a compile-clean-but-still-wrong fix is PROBE-FAIL, not a
candidate. Items without a PROBE line degrade to compile-only CANDIDATE, as
before. So the loop is: mini fixes -> clippy -> boot patched server -> diff vs
vanilla -> VERIFIED.

Honest state: the machinery is connected and proven, but ZERO queue items have
PROBE recipes yet, so nothing is behaviourally verified until they are written.
That is the real remaining work (task 22) and it is per-behaviour: a growth-rate
fix needs a setblock -> force ticks -> query-age sequence, lighting needs a
debug query, etc. The gate activates the moment a recipe exists. Also note the
spree process launched earlier runs the OLD compile-only code; the gate applies
next run.


## 2026-07-29 - Storage: one shared build cache, not one per worktree

Audit found the only real disk cost is cargo target/ dirs: 19 worktrees at
~3.4 GB each, ~65 GB of duplicated build cache. Everything else the loop
writes (candidate diffs, DISCOVERED.md, logs, mini trajectories) is kilobytes.

Fixes: the loop now exports CARGO_TARGET_DIR=~/pumpkin-local/.loop-target, so
mini and verify share ONE build cache bounded at ~4 GB regardless of worktree
count - target dirs no longer multiply. verify.py deletes its throwaway
.verify-server world after each run. cycle.log self-trims to 3000 lines.
Reclaimed ~43 GB by clearing the retired bespoke-harness worktrees' target
dirs (pure regenerable cache; candidate diffs were already saved to
candidates/). Left the active spree worktree's target alone. Free: ~608 GB.


## 2026-07-29 - The spree was thrashing, not working: swap + orphan bug

Status bar showed 0/12 and the honest reason was ugly. A1 and A2 both timed
out at 40min with no edit, and two mini processes were running at once against
the single model slot. Two causes:

1. The box was swapping. Terraria + Steam + the 16.5 GB model exceed 30 GB
   RAM; 7-13 GB in swap dropped the model from ~3 to 0.7 tok/s. Same swap
   incident already in this file, third time. It is environmental - the loop
   should run when the box is idle (the 2am nightly), not while gaming. Not
   "fixed" by closing the user's games.
2. Orphan bug: mini_spree used subprocess.run(timeout=), which kills only the
   bash shell, not the mini child. Timed-out minis kept running and stacked
   up. Fixed with run_pg(): start_new_session + os.killpg on timeout kills the
   whole descendant group. Bumped per-item timeout 40->60min for idle-box runs.

Also worth saying plainly: the A-series (lighting math, multi-file skyDarken
tracking) may be too hard for mini on a 3 tok/s local model regardless of
timeout. The trivial nether-wart smoke test worked; these did not. Expect the
local spree to land the simple confirmed items and stall on the hard ones -
those are the ones to route to review or a stronger model. The nightly will
try them on an idle box and log the outcomes; nothing is lost.


## 2026-07-29 - Discovery and spree run in parallel

parity_cycle.sh now backgrounds oracle_discover.py and starts mini_spree
immediately, waiting for discovery only at the end. Safe because discovery is
pure RCON to the NUC servers and never touches the local model, so there is no
GPU contention with the spree. Today the win is small (discovery is minutes,
the spree is hours), but the structure scales: as probe categories grow
(task 22 - blocks, items, mobs), a long discovery sweep now fully overlaps the
spree instead of serializing in front of it.


## 2026-07-30 - Automated discovery review, held to a safe line

The queue-growth step is now automated but deliberately narrow.
triage_discoveries.py classifies each oracle discovery from its observed
evidence: NOISE-COSMETIC (error-wording only), NOISE-CONFIG (a value like
max-players), FEATURE (a whole command/system Pumpkin lacks), or FIX (a small
localizable behaviour gap). Noise is dropped, features are parked to
DISCOVERED-FEATURES.md as design work, and ONLY FIX-class items are eligible
to promote - and even those go in as SUSPECTED, needing a code locus confirmed
before the spree touches them.

Run on today's 22 discoveries: all 22 classified FEATURE (missing commands),
0 FIX, 0 promoted. That is the correct, honest outcome - they are whole
features, not spree-sized fixes, so nothing floods the queue. Wired into
parity_cycle.sh after discovery. As probe categories grow to value-level gaps
(gamerule drift, block-state results), those surface as FIX and flow through
the same gate. The one thing still NOT automated, on purpose: turning a FIX
discovery into a spec with a real WHERE/DO code locus. That is the judgement
the project pays for guessing, so it stays a confirm step.


## 2026-07-30 - Source-coverage enumeration hit a reliability ceiling

Built coverage.py to walk the block registry and list unimplemented behaviour
families (the "go through source one by one, add features to translate" idea,
done data-driven so it never translates Minecraft code - clean-room holds).
It usefully collapses ~1000 gap blocks into a few interactive families.

But it is not trustworthy. Four successive parsers all false-flagged
IMPLEMENTED families as gaps: (1) macro-only detection missed non-#[pumpkin_block]
registration; (2) case sensitivity missed DoorBlock; (3) \bdoor\b never matches
oak_door (underscore is a word char); (4) registry-gated declaration matching
still missed pressure_plate/skull/cauldron, which registry.rs proves are
registered. The block->behaviour mapping (registry.rs + attribute macros + tag
expansion + nested modules) has too much variation for static parsing.

Only concrete_powder was a genuine gap under every method. Everything else on
the coverage list is suspect. Lesson, same family as the "machine was swapping"
and "NUC unreachable" entries: verify before believing a tool's output. The
file carries a RELIABILITY WARNING header. The trustworthy gap-finder remains
the runtime oracle, which observes behaviour rather than inferring it from
source patterns. coverage.py stays as a weak hint generator, not a roadmap, and
is NOT wired into the auto-loop.


## 2026-07-30 - Behavioural differential oracle: built, working, honest guard

Expanded the oracle from commands to BLOCK BEHAVIOUR. nuc_behavior.py places a
scenario on both servers, waits, then reads the result cell on both and diffs.
Getting there meant discovering (live) how thin Pumpkin's introspection is and
reusing the pumpkin-bot rig's tricks:
  - Pumpkin loads no chunks over RCON and its forceload is broken, so
    pumpkin-bot/holder.js (raw protocol client) holds spawn loaded. Vanilla
    uses forceload (works there).
  - Pumpkin has no `data get` / `execute`, so block state is read by
    `clone p p dst filtered <block>` and parsing the count. Wording differs:
    vanilla "Successfully cloned N block(s)", pumpkin "N blocks cloned" - the
    parser handles both.
These Pumpkin gaps (setblock rejects block-state args, no execute, no data get,
broken forceload) are themselves real command-parity findings from the probe.

The correctness rule that makes it trustworthy: a gap is filed ONLY when
vanilla did the expected thing (validating the probe) AND Pumpkin differs. This
caught its own noise - an early run filed door_double and sand_falls where
vanilla itself did not behave as expected (setblock bypasses placement logic;
timing off). Those were removed and the guard now refuses them. Current battery:
sapling_stays passes (parity), sand/snow need timing tuning, 0 false gaps filed.

Not yet wired to the nightly: it needs the holder bot running and the probes
need better timings/conditions (task 22). setblock-triggered effects (physics,
random ticks, light) are testable; item-use placement (door halves, bed heads)
needs the bot to right-click, which is the pumpkin-bot rig's job. Holder bot
was stopped after the run - it reconnect-spams every 30s and should only run
while the behavioural oracle is actively probing.


## 2026-07-30 - Behavioural oracle wired; RCON physics probes have a ceiling

Wired nuc_behavior.py into parity_cycle.sh step 4: start the holder bot (loads
Pumpkin spawn), wait 22s, run the probes, kill the bot. Guarded so it is safe
even with an immature probe set - a gap is filed only when vanilla reproduces
the expected behaviour AND Pumpkin differs.

Honest finding from actually tuning the probes: RCON `setblock` does not kick
off physics block-updates the way real gameplay does. Vanilla itself would not
reproduce sand-falling, snow-melting, water-spread, or ice-melting on command
in-window, so the guard correctly filed nothing. Conclusion: physics behaviours
belong in the mineflayer rig the user already built (leaftest.js, crittest.js -
a real client that places/breaks blocks), NOT in RCON clone probes. The RCON
behavioural oracle is the right tool for command/global/placement-level gaps
(it already found: setblock rejects block-state args, no execute, no data get,
broken forceload) and for tick effects that setblock does trigger. sapling_stays
passes as a parity control.

So the engine and its safety guard are done and live; the probe LIBRARY is the
ongoing work, split by tool: RCON for state/command/placement, the bot rig for
interaction physics. Wiring the bot-rig probes (leaftest/crittest) into the same
guarded differential shape is the natural next step, not more RCON physics.


## 2026-07-30 - Behavioural probes tuned; live run blocked on a flaky chunk-holder

Corrected the earlier "RCON cannot do physics" conclusion: it can do
RANDOM-TICK physics (leaf decay, ice melt) given long waits + random_tick_speed
cranked - leaftest.js waits up to 210s; my first pass waited 18s. Rebuilt the
probe battery around random-tick behaviours (leaves_decay_lone,
leaves_keep_near_log, ice_melts_by_torch, sapling control) with 60-75s waits.

But live execution is flaky: holder.js reconnects every ~30s (protocol 776 vs
node-minecraft-protocol's 1.21 ceiling, keep_alive does not round-trip), and
during each gap Pumpkin's spawn chunk unloads, which stalls or -1's any probe
mid-wait. A 75s probe spans 2-3 reconnect gaps and is unreliable. So the
behavioural oracle needs a STABLE chunk-holder (a client that stays connected,
i.e. node-minecraft-protocol updated for 776, or Pumpkin fixing forceload)
before it runs reliably. The engine, guard, tuned probes and cycle wiring are
all in place; the blocker is infrastructure stability, honestly not solved.

Not claiming and will not claim "90-95% vanilla coverage." That is years of
work for a from-scratch server; a session builds the engine that pursues it, it
does not arrive. Overclaiming here is the exact mistake this file exists to
prevent.


## 2026-07-30 (morning) - The behavioural step hung the nightly for 6 hours

Wiring the flaky behavioural oracle into the cycle was a mistake, exactly as
the risk noted the night before. The 2am cycle ran the spree fine (banked D2
and F2, ~5/12 confirmed items now have candidates), discovery found 0 new (22
dedup), then HUNG on the behavioural-oracle step for 6 hours - the holder bot's
30s reconnect stalled nuc_behavior with no timeout, and the whole oneshot
service sat in "activating" while process copies piled up.

Fix: the behavioural step is now opt-in (COVER_BEHAVIOUR=1, default off) AND
wrapped in `timeout 360`, with an unconditional bot kill. So the nightly can
never again be blocked by it, and no holder bot is left reconnect-spamming. Run
the behavioural oracle by hand while watching until a stable chunk-holder
exists. Lesson restated: do not put an unproven, flaky component in the
unattended path. The spree + command oracle + review are proven; those stay.

## 2026-07-30 - Maintainer feedback: atomic PRs, and two closures

Snowiiii closed #2612 with "use atomic PRS" and closed #2587 without comment.
Response, same day: #2612 split into #2655 (file logger falls back to console)
and #2656 (level.dat backup failure is nonfatal), each one fix in one file. The
third piece, Empty(15) sky light across save, no longer compiles against master
a3b66be because the chunk serialization structs changed, so it needs a real port
rather than a file copy and follows separately.

#2587 (random_tick_speed) is NOT superseded: the rule is still unwired on
current master and #2575 does not touch it. Asked in a two-sentence comment
whether a rebased minimal version is welcome rather than reopening unilaterally.

Standing rule going forward: one fix, one file where possible, one PR. The
already-open bundles (#2589, #2602, #2611, #2590) stay as they are unless a
maintainer objects, but nothing new ships bundled.

## 2026-07-30 - Minestom and Cuberite adopted as behaviour references

Shallow clones at ~/pumpkin-local/refs/minestom (Java, close to the modern
protocol) and ~/pumpkin-local/refs/cuberite (C++, mechanics written out
plainly). The scaffold design pass is pointed at them.

Behaviour reference only, never code translation. Three reasons, each
sufficient: different languages, so translation invites subtle semantic drift;
different licenses, so copying is not clean either way; and both are
reimplementations with their own bugs, so neither outranks the oracle. Their
value is decomposition and ordering: how another team split a subsystem, what
state it keeps, what happens first. Where a design choice comes from one of
them, the spec or PR says so.

## 2026-07-31 - 11 PRs vanished: a pipeline deleted fork branches, which auto-closes PRs

Eleven PRs (#2677-#2687) showed as closed "by eshankiyer" within a 12-second
window, which read as the user closing them. They did not touch anything. The
cause: something deleted those branches from the eshankiyer/Pumpkin fork, and
GitHub auto-closes any PR the moment its head branch disappears, attributing the
close to whatever account owns the token. So an automated branch cleanup on the
fork silently closed a dozen open PRs.

This is a landmine in the shipping automation. sync.py's retire_merged only
deletes branches of MERGED PRs, but SOME process deleted branches of OPEN ones.
Until the culprit is identified, NO automation may delete a fork branch whose PR
is still open. Rule: before deleting any branch, check `gh pr list --head
<branch> --state open`; if a PR references it, never delete.

Recovery: the commits and local branches all survived. Rebased each onto current
master, re-pushed (plain push, not force-with-lease, because the remote ref was
gone so there was nothing to lease against), and opened fresh PRs. You cannot
reopen a PR whose branch was deleted.

Three needed a build fix from the rebase onto today's master:
- firework-payload (#2687->#2697): upstream renamed the `item` param to `_item`
  (unused); this branch actually uses it, so renamed it back.
- elytra-firework (#2684->#2695): `items_after_test_module` denied; moved the
  `#[cfg(test)] mod tests` to the end of the file.
- pvp-disabled-pve (#2678->#2689): `doc_markdown` denied; backticked `PvP`.

Excluded #2686 (skeleton ranged combat) on the user's instruction; RoosterBooster
had already flagged it as similar to #2352.

## 2026-07-31 - /tmp is a 16 GB RAM-disk, do not build there

The parallel session put ~25 per-PR git worktrees and their cargo target dirs
under /tmp, which is tmpfs (RAM). It filled to 100%, and cargo failed with "No
space left on device" while df showed 520 GB free on the real disk. The full
RAM-disk also starves the model. Reclaimed 16 GB by `git worktree remove`-ing
all /tmp worktrees. All pipeline builds go under /home/eshanki/pumpkin-wt on the
real disk, never /tmp.
