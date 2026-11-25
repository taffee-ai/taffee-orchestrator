# Conformance vectors

Each vector feeds a context and an input to one function of the orchestrator and compares what comes out.

| File | Function | Input → output |
|---|---|---|
| [`normalize.yaml`](normalize.yaml) | normalize | request (usually a dry run) → reply (READY or REFUSED) |
| [`handle.yaml`](handle.yaml) | admit | request → REFUSED, ACCEPTED, or `runs: true` with the digest that would run |
| [`result.yaml`](result.yaml) | assemble | frozen manifest + simulated work output → reply |
| [`delegate.yaml`](delegate.yaml) | delegate | parent frozen manifest + child request → the child frozen manifest, or a refusal |

Every file reads [`context.yaml`](context.yaml): the registry, name resolution, policy and the fixture repository. A vector's own `context` is merged over it.

## Matching

`expect` is a subset: every field listed must match, and fields not listed are not compared.

| Operator | Matches |
|---|---|
| a literal | an equal value (maps compared as subsets, lists exactly) |
| `$any` | the field is present, with any value |
| `$absent` | the field is not present |
| `{ $match: <regex> }` | a string matching the regex |
| `{ $same: <vector>.<path> }` | equal to that value in another vector's output |
| `{ $differs: <vector>.<path> }` | not equal to it |
| `{ $contains: [..] }` | a list or map containing at least these items or keys |

An input can be built from another vector's output: `{ $from: <vector>.<path>, with: {…} }` takes that value and merges `with` over it. If the value was frozen, the harness recomputes the digest after merging, unless `with` sets `digest` itself (R-06 does this to build a tampered manifest).

A normalizer that uses a model runs every `normalize` vector 5 times, and every run must match.
