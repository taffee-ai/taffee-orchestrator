# Work manifest 0.1

**Caller ⟷ `/` ⟷ Orchestrator.** This spec is the `/`: what crosses between whoever asks for work and the agent orchestrator that does it.

## Why this spec exists

An orchestrator, meaning an agent orchestrator loop, takes a request, plans it, runs agents, tools and models, and hands back a result. Everything inside that loop belongs to whoever builds it.

Two things don't: what a caller has to say to get work done, and what it can count on getting back. Most callers will never read a spec. They type a sentence. Others, like programs and other orchestrators, need something exact. Without a shared form, every orchestrator invents its own intake, and nobody can tell whether a result actually meets the request.

This spec is that shared form, and nothing else.

## Who it is for

| For | Not for |
|---|---|
| People who build an orchestrator | People who build or tune models |
| People and programs that send work to one | How an orchestrator plans, or which agents and models it runs |
| Orchestrators that hand work to other orchestrators | How messages travel (HTTP, queues, agent protocols) |

**It assumes nothing about the models.** They may answer differently on each run, be wrong, or be replaced. The promises have to hold anyway, so they rest only on what the orchestrator can check: the frozen manifest, the pinned inputs, and checks that code evaluates.

## How our orchestrator works, and why the spec looks the way it does

```
                         ┌───────────────────────/────── ORCHESTRATOR ───────\───────────────────────┐
                         │                       /                           \                       │
                         │                       /                           \                       │
┌──────────────┐         │  ┌──────────────┐     /     ┌──────────────┐      \     ┌──────────────┐  │
│ 1 CALLER     │ request │  │ 2 NORMALIZER │     /     │ 4 OUTER PLAN │      \     │ 6 INNER LOOP │  │
│              │─────────┼─▶│              │─────/────▶│              │──────\────▶│              │  │
│ person       │         │  │ intake       │     /     │ heavy        │      \     │ workers      │  │
│ CLI · API    │         │  │ translates   │     /     │ runs once    │      \     │ light        │  │
│ agent        │         │  └──────────────┘     /     │ decides how  │◀─────\─────│ repeated     │  │
│ orchestrator │◀────────┼───────────────────────/─────│              │reply \     │              │  │
└──────────────┘ reply   │                       /     └──────────────┘      \     └──────────────┘  │
                         │                       /                           \                       │
                         │                       /                           \                       │
                         └───────────────────────/───────────────────────────\───────────────────────┘
                                            3 the spec                5 the same spec
```

1. **A caller asks.** It can be a person, a CLI, an API client, an AI agent or another orchestrator. It doesn't need to know this spec. A sentence is enough.
2. **The normalizer takes it in.** This is the orchestrator's intake. It picks the intent kind, fills in the parameters, pins the inputs to exact versions and records every interpretation in `assumed`. If there's a gap it can't fill, the caller gets a refusal with questions, never a guess.
3. **The request crosses `/` as a manifest.** The manifest is frozen: complete, pinned and digested. This is the line the spec defines. A caller that already speaks the spec sends a manifest, and the normalizer only checks it.
4. **The outer plan works from the manifest alone.** It is heavy and runs once. It sees only the fields in the digest (what is wanted, the inputs, the completion conditions, the limits), never the caller's wording or identity. It decides how to do the work, and it can't change what was asked.
5. **The outer plan hands work to the inner loop in the same form, `\`.** Each worker gets its own manifest, with narrower limits, its own checks and its parent's digest. The outer plan is a caller to its workers, and the same rules apply: delegation only narrows.
6. **Workers run, and the orchestrator checks.** The reply goes back across `\`, then `/`. The orchestrator evaluates every check itself, so a worker can't declare itself done.

Steps 3 and 5 are what the spec requires. The rest is how the taffee orchestrator does it. Another orchestrator may work differently inside, as long as what crosses `/` and `\` follows the spec.

**Why the spec is shaped this way:**

| Because | The spec has |
|---|---|
| Callers don't know the spec | sparse requests (a sentence is valid), with `said` and `assumed` so every interpretation can be audited |
| Some callers can't hold a conversation (API clients) | one request and one reply; `if_unclear`, `approved` and `dry_run` decided up front |
| The plan must not depend on wording | a digest that leaves out who asked and how they phrased it; the planner sees only the digested fields |
| Workers are models, and can be wrong | completion decided by checks the orchestrator runs, not by what a worker claims |
| Work gets handed on (outer plan to workers, orchestrator to orchestrator) | one form at every hand-off, and each hand-off can only narrow |

## What the spec asks of callers

The spec also shapes how callers think. It asks the questions any orchestrator has to answer before it can promise anything: what outcome, from which inputs, done when, and within what limits. A caller who writes a plain sentence still sees those questions answered, in `assumed`, in the readback, and in the questions a refusal sends back. Over time callers learn to ask in that shape, and every request becomes practice in writing a contract.

## What a reply promises

1. **The same request runs the same work.** Every request is turned into a frozen manifest, and its digest names the work.
2. **An answer comes back only when it has been checked against the request.** The orchestrator runs the checks, not the worker that produced the answer.
3. **Anything else comes back as a reason from a fixed list.** The caller never gets a guess.

Code is the source of truth here: the schemas, [`rules.yaml`](rules.yaml) and [`vectors/`](vectors/). This page is the human summary of them.

## The smallest request

```yaml
manifest: 0.1
want: If I rename PaymentService to PaymentGateway in payments-service, what breaks? Need file and line.
```

`by` comes from authentication. This work is read-only, so it runs without anyone confirming it. The reply:

```yaml
reply: 0.1
status: COMPLETED
digest: sha256:3b7f…
assumed:
  kind: '"what breaks" read as code.impact_analysis@1'
  given.repo: '"payments-service" is github.com/acme/payments-service; main is 4f2c9e1'
checks: { deliver: pass, cited: pass, on_target: pass, complete: pass }
answer:
  breaks:
    - { file: src/api/routes.ts, line: 2, reason: import }
    - { file: src/api/routes.ts, line: 14, reason: call }
    - …
  mentions:
    - { file: docs/README.md, line: 12, where: doc }
usage: { usd: 0.12, time: 94s }
```

The caller can tell exactly what was understood from `assumed`. The values in `checks` say why the orchestrator counts the work as done.

## The frozen manifest

The same request after normalization. The top part is for people and is what a chat front end shows as the readback. The bottom part is for code.

```yaml
manifest: 0.1
# ── for people ──
by:     user:alice
said:   If I rename PaymentService to PaymentGateway in payments-service, what breaks? Need file and line.
want:   Every place that breaks if PaymentService is renamed to PaymentGateway
given:  { repo: github.com/acme/payments-service@4f2c9e1b7a3d5c8e9f0a1b2c3d4e5f6a7b8c9d0e }
done_when:
  cited:     Every place cites a file and line that exist at the pinned commit
  on_target: Every cited line mentions the symbol
  complete:  Every source file was searched
limits: { effects: read-only, usd: 0.50, time: 10m }
assumed:
  kind:       '"what breaks" read as code.impact_analysis@1'
  given.repo: '"payments-service" is github.com/acme/payments-service; main is 4f2c9e1'
# ── for code ──
kind:       code.impact_analysis@1#sha256:…
parameters: { symbol: PaymentService, rename_to: PaymentGateway, paths: ["**"] }
decide:     { non_code_mentions: list_as_mentions, dynamic_references: list_as_breaks, tests: include, generated_code: include }
deliver:    { schema: code.impact_analysis@1/deliver#sha256:…, order: [file, line], equivalence: { breaks: "set(file, line, reason)", mentions: "set(file, line)" } }
requires:   { declared: "declared(given.repo, parameters.symbol)" }
checks:
  cited:     "(answer.breaks + answer.mentions).all(i, line_exists(given.repo, i.file, i.line))"
  on_target: "(answer.breaks + answer.mentions).all(i, line_mentions(given.repo, i.file, i.line, parameters.symbol))"
  complete:  "coverage.missed.size() == 0 && coverage.searched == coverage.total"
partial_ok: false
delegation: { depth: 0 }
digest:     sha256:3b7f…
```

## Fields

**In digest** means the field defines the work: same digest, same work. The digest leaves out who asked, how they phrased it and how they want the reply.

| Field | Who writes it | In digest | Meaning |
|---|---|---|---|
| `manifest` | caller | ✓ | spec version, `0.1` |
| `by` | set from authentication | | who is asking |
| `for` | delegating orchestrator | | on whose behalf, oldest first |
| `want` | caller (or `kind`) | ✓ | the caller's words; once frozen, the kind's restatement |
| `said` | normalizer | | the caller's original `want`, verbatim |
| `why` | caller, optional | ✓ | the purpose; the planner uses it for judgment calls |
| `given` | caller or normalizer | ✓ | named inputs; pinned once frozen |
| `done_when` | kind, and the caller may add | ✓ | readable completion conditions, by id |
| `limits` | kind default or caller, capped by policy | ✓ | `effects` · `usd` · `time` |
| `assumed` | normalizer | | every interpretation, by field path |
| `kind` | normalizer or caller | ✓ | the intent kind; pinned once frozen |
| `parameters` | normalizer or caller | ✓ | the kind's parameters |
| `decide` | kind default, or the caller picks from the kind's options | ✓ | answers to questions that may come up during the work, decided in advance |
| `deliver` | kind | ✓ | answer schema (pinned), order, equivalence |
| `requires` | kind, and the caller may add | ✓ | preconditions; one false → `NOT_FOUND` |
| `checks` | kind, and the caller may add | ✓ | one per `done_when` id: a CEL expression or a pinned verifier |
| `partial_ok` | kind default or caller | ✓ | whether an incomplete answer may come back |
| `delegation` | kind default or caller | ✓ | `depth`: how many more hand-offs are allowed |
| `parent` | delegating orchestrator | ✓ | the parent manifest's digest |
| `if_unclear` · `approved` · `dry_run` · `reply` · `id` | caller | | how to handle this request (next section) |
| `digest` | orchestrator | | present means frozen |
| `x-*` | anyone | | ignored, never affects the work |

## Handling a request without a conversation

| Field | Values | Default | Effect |
|---|---|---|---|
| `if_unclear` | `assume` · `refuse` | `assume` for read-only work, `refuse` otherwise | `assume`: run with the interpretations recorded in `assumed`. `refuse`: if anything beyond pinning a version had to be interpreted, refuse with questions |
| `approved` | `read-only` · `reversible` · `irreversible` | `read-only` | the highest effect allowed to run; anything above it is refused, never left waiting |
| `dry_run` | `true` | `false` | return the frozen manifest (`READY`) and run nothing |
| `reply` | `wait` · `receipt` | `wait` | `receipt`: return `ACCEPTED` with an id, and fetch the final reply later |
| `id` | string | none | the caller's request id; resending it gets the same reply |

**Confirmation takes two independent requests.** Send `dry_run: true` and get back the frozen manifest. To confirm, send that manifest back with `approved` set. The digest connects the two requests; there is no session.

A gap is a required value that can't be filled from the request, the kind or name resolution. It is always refused, whatever `if_unclear` says.

## Replies

| Status | When | Comes with |
|---|---|---|
| `READY` | dry run | `manifest` (frozen), `assumed` |
| `REFUSED` | did not run | `reason`, `violates` (rule ids), and `questions`, `counter` or `manifest` |
| `ACCEPTED` | `reply: receipt` | `receipt` |
| `COMPLETED` | every check passed | `answer`, `checks`, `usage` |
| `PARTIAL` | some checks didn't pass, and `partial_ok` is true | `answer`, `checks`, `usage` |
| `NOT_FOUND` | a precondition (`requires`) was false | `missing` |
| `INDETERMINATE` | nothing failed, but some check couldn't be evaluated | `checks` |
| `FAILED` | anything else | `failure.code`, `checks` if the work ran |
| `CANCELED` | cancelled after `ACCEPTED` | |

The closed set of `REFUSED` reasons: `unclear` · `needs_approval` · `out_of_policy` · `over_budget` · `unsupported` · `invalid`.

The closed set of `FAILED` codes: `checks_failed` · `timeout` · `budget_exhausted` · `input_unavailable` · `effect_denied` · `internal`.

An `answer` comes back only with `COMPLETED` or `PARTIAL`. Follow-up work the orchestrator suggests comes back as `proposals`, each one a sparse manifest. The orchestrator never runs a proposal on its own.

## The rules that carry the design

1. **Only frozen manifests run.** Frozen means it has a digest, every input and reference is pinned, and it can be run without looking anything up in the registry.
2. **The digest names the work, not the request.** A person's sentence and a script's parameters that mean the same work get the same digest.
3. **The planner sees only the fields in the digest.** The original wording, the caller's identity and the handling fields never reach it.
4. **The orchestrator evaluates the checks.** A worker can't declare itself done. `COMPLETED` means every check passed.
5. **Nothing waits for a human.** A gap or a request for more approval becomes a refusal that says exactly what to change.
6. **A caller can only narrow.** It can add checks, never remove them. Limits it doesn't set are capped by policy, and limits it sets above policy are refused. A delegated manifest has to fit inside its parent's.

## Files

| File | What it is |
|---|---|
| [`manifest.schema.json`](manifest.schema.json) | the request, sparse and frozen (JSON Schema 2020-12) |
| [`reply.schema.json`](reply.schema.json) | the reply |
| [`kind.schema.json`](kind.schema.json) | the format of an intent kind |
| [`kinds/code.impact_analysis@1.yaml`](kinds/code.impact_analysis@1.yaml) | the reference kind |
| [`rules.yaml`](rules.yaml) | every rule: id, level, one-line text, and how it is proved (vectors, schema or audit) |
| [`vectors/`](vectors/) | conformance cases: normalize, handle, result, delegate |
| [`fixtures/`](fixtures/) | the test repository the vectors use |

**Conformance:** an orchestrator conforms if it passes every vector cited by a MUST rule. The taffee orchestrator's harness is `runner/tests/conformance.rs`. Vectors for an LLM-based normalizer have to pass on every run, not just on average.

## Not in 0.1

- Limits on where data may go (for example, "don't send my code to provider X").
- Progress reports while work runs.
- Cancel as an operation (the `CANCELED` status is reserved for it).
- Who governs the kind registry, and how kinds are published.
- The canonical form of YAML input (0.1 digests the JSON data model, so YAML input hashes the same as JSON input).
