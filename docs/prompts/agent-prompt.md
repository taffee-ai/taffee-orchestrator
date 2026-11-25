Research: An intent-based request/result contract for agent systems
Independent standards assessment · 15 November 2026
Executive assessment
Preliminary conclusion: compose existing standards, with a small, vendor-neutral contract specification for the semantics they do not cover. No single established specification I can identify defines the entire request-to-verifiable-result contract.

The important distinction is between three different guarantees:
1. Structural determinism: the request and result conform to known schemas.
2. Acceptance determinism: independent evaluators reach the same pass/fail decision using the same evidence and rules.
3. Semantic determinism: independent executions produce equivalent correct outcomes.
The first two can be engineered through a contract. The third requires more: stable input snapshots, sufficiently precise outcome definitions, complete observation, deterministic verification, and a defined equivalence relation.

A contract alone cannot make an incomplete search complete or an incorrect result correct.

The strongest architectural direction is therefore:

Intent specification + pinned execution context + machine-verifiable acceptance + evidence-backed result.

This is closer to design by contract combined with declarative desired-state specification and proof-carrying results than to conventional prompt engineering.

The research below separates established standards from proposed extensions. A specification's popularity does not imply that it provides deterministic semantics.
1. Survey of published candidates
The initial field survey identified the following families independently of the appendix. I then checked the appendix and incorporated its additional candidates.
A. Requirements, contracts, constraints and testing
Candidate	Contract elements covered	Assessment
ISO/IEC/IEEE 29148	Requirements, verification, traceability	Strong engineering discipline; not a runtime contract. The 2018 edition remains published while a 2026 revision is in draft.  Requirements engineering
Eiffel Design by Contract	Preconditions, postconditions, invariants	Closest conceptual foundation; no standardized agent result envelope.
Gherkin / BDD	Preconditions, expected behavior, executable examples	Excellent human-readable acceptance language; actual checks depend on step definitions.  Cucumber
JSON Schema 2020-12	Input/output structure, type constraints, required fields	Strong structural contract, but cannot establish semantic correctness or completeness.  JSON Schema
CUE	Constraints, validation, composition	Powerful alternative to JSON Schema with formal unification semantics.  CUE
OPA / Rego	Policy predicates, permissions, acceptance rules	Strong machine-evaluable logic over structured inputs.  Open Policy Agent
Common Expression Language (CEL)	Boolean acceptance expressions, constraints	Particularly promising for portable, bounded validation expressions.
RFC 9457	Structured failure details	Reusable error pattern; HTTP-oriented and does not establish a closed application-specific error taxonomy.  RFC Editor

Additional relevant methods include EARS requirements syntax, property-based testing, formal methods such as TLA+, and executable specification techniques. These provide requirements discipline or verification methods, not complete external request/result contracts.
B. Intent, desired-state and agent specifications
Candidate	Contract elements covered	Assessment
TM Forum TR299 and TR290 family	Intent semantics, desired state, reporting, lifecycle	Most relevant standards-family comparison for intent itself. Domain-specific and with licensing concerns.  TM Forum+1
3GPP TS 28.312	Intent management, conflict handling, reporting	Mature telecom intent model, not a general agent work contract.  3GPP Portal
Oracle Open Agent Specification	Agent/flow definitions, configuration	Primarily describes the agent system rather than the external obligation.  GitHub
GitHub Spec Kit	Requirements, planning, validation workflow	Useful methodology; does not establish universal request/result semantics.  GitHub
Agent Skills	Capability description, procedural instructions	Describes reusable skills, not verifiable outcomes.  GitHub
DSPy Signatures	Typed input/output intent and instructions	Concise interface abstraction; limited contract guarantees.
Kiro Specs	Requirements, design, tasks	Specification-driven development method.
Strands Agent SOPs	Procedural guidance	Operating instructions rather than an external acceptance contract.
A2A task semantics	Task identity, lifecycle states, artifacts	Useful result/lifecycle vocabulary, but completion does not prove acceptance.  A2A Protocol

C. Result structure, evidence and interchange
Candidate	Contract elements covered	Assessment
OpenAPI	Request/response schemas, errors	Strong API contract, but operation-oriented rather than outcome-oriented.  OpenAPI Initiative Publications
Arazzo 1.1	Workflow inputs, dependencies, success criteria, outputs	Important additional candidate; promising but prescribes API workflows, which are outside the intended contract.  OpenAPI Initiative Publications
W3C PROV	Evidence lineage, entities, activities, agents	Strong standardized provenance vocabulary. Does not prove the truth of a claim.  W3C
in-toto Attestation	Subject identity, digests, typed evidence statements	Strong evidence packaging and integrity foundation.  GitHub
SLSA Provenance	Inputs, build provenance, reproducibility metadata	Valuable precedent, but focused on software supply chains.
CloudEvents	Event identity, source, type, metadata	Useful envelope, not work semantics.  GitHub
Provider structured outputs	Constrained result schema	Helps enforce syntax, not semantic validity; vendor implementations differ.  OpenAI API

Also relevant are JSON Pointer (RFC 6901), JSON Patch (RFC 6902), JSON Canonicalization Scheme (RFC 8785), and W3C Verifiable Credentials. They address referencing, change representation, stable hashing, and signed assertions respectively.

Appendix reconciliation: The appendix added useful emphasis on EARS, TM Forum, 3GPP, Oracle Agent Spec, Kiro, and Strands. The independent survey additionally surfaced CEL, CUE, Rego, Arazzo, RFC 9457, in-toto, SLSA, and formal verification methods. I found no complete solution among these candidates.
2. Coverage matrix
Legend: F = fully covers the indicated capability within its intended scope; P = partially; — = not covered.

To keep the matrix readable, candidates are grouped into their strongest representative families. These are analytical ratings, not conformance claims.

Core request and result semantics
Contract element	29148 / DbC	TMF / 3GPP	JSON Schema / CUE	CEL / Rego	Gherkin
Desired outcome	F	F	P	P	P
Preconditions	F	P	P	F	F
Pinned inputs	P	P	P	P	P
Deliverable structure	P	P	F	P	P
Acceptance predicates	P	P	P	F	F
Ambiguity policy	P	P	P	P	P
Closed failure meanings	—	P	P	P	—
Boundaries and budget	P	P	P	F	P
Authority and approval	P	P	P	F	—
Evidence and lineage	P	P	P	P	P
Stable identity/replay	P	P	P	—	P
Equivalence/completeness	P	P	P	P	P

Supporting specifications
Contract element	OpenAPI / Arazzo	PROV / in-toto	A2A states	Spec Kit / Kiro	DSPy / Skills
Desired outcome	P	—	P	F	P
Preconditions	P	—	—	P	P
Pinned inputs	P	F	P	P	P
Deliverable structure	F	P	P	P	P
Acceptance predicates	P	—	—	P	—
Ambiguity policy	—	—	P	P	P
Closed failure meanings	P	—	P	—	—
Boundaries and budget	P	—	—	P	P
Authority and approval	P	P	P	P	—
Evidence and lineage	P	F	P	P	—
Stable identity/replay	P	F	P	P	—
Equivalence/completeness	P	—	—	P	—

An important qualification: F for provenance means the standard can represent provenance, not that it guarantees truthful or complete provenance. Similarly, F for acceptance predicates means the language can evaluate an expressed predicate, not that the predicate captures the requester's true intent.
3. Decision
Compose standards; define a minimal new contract profile. Do not invent a new prompt language.
Recommended contract stack
Intent Contract v0.1 — project-defined

Outcome · equivalence · ambiguity · authority · failure semantics


JSON Schema 2020-12

Request and result structure


CEL

Portable acceptance predicates


W3C PROV

Evidence and source lineage


in-toto

Digest-bound attestations



Existing standards provide the mechanisms. The project-defined profile supplies the missing semantics and conformance rules.
CEL is my preferred initial acceptance language over Rego because a restricted, side-effect-free expression profile is easier to embed in a portable contract. Rego remains a strong alternative for richer organizational policy. This is an architectural judgment, not a proven cross-runtime portability benchmark.
Minimum project-defined elements
Field	Meaning
contract_version, request_id	Versioned semantics and identity
intent	Desired outcome, target, scope
inputs	Immutable references and digests
output	Schema reference, ordering and canonicalization
acceptance	Named predicates and verifier versions
ambiguity	Deterministic resolution or rejection
constraints	Allowed effects, resources, budget
authority	Principal, permissions, approval requirements
failure_policy	Closed result states and partial-result rules
evidence_policy	Required citations, coverage and provenance
equivalence	Rules determining whether two results are equivalent

The result mirrors the contract through request_id, status, artifact, checks, evidence, assumptions, usage, and failure.

Two additions are particularly important:
- Completeness certificate: how the system establishes that it found every matching item, rather than merely finding plausible items.
- Verifier identity: exact versions and digests of the logic used to accept or reject the result.
A machine-checkable predicate cannot independently prove completeness unless it has an adequate reference set, exhaustive analysis method, or other sound completeness argument.
Licensing and exit cost
Component	License assessment	Exit cost
JSON Schema	Open specification; implementation licenses vary	Low
CEL	Apache-2.0 reference ecosystem; verify each implementation	Low–medium
W3C PROV	W3C specification terms; check redistribution obligations	Medium
in-toto	Open specification; implementation and documentation terms vary	Low–medium
Custom contract profile	Publish under Apache-2.0 with explicit contribution terms	Under project control
TM Forum TR299	Published IPR mode RAND; not sufficient to establish unrestricted Apache compatibility	Potentially high

TM Forum's official TR299 page explicitly lists RAND as its IPR mode. That does not prove a royalty obligation, but it is enough to require further legal review before treating it as an unrestricted normative dependency. 

TM Forum

4. Strongest case against this decision
The strongest runner-up is CUE as the primary contract language, with a thin result envelope.

CUE combines schema constraints, values, defaults, composition and validation within one formally specified language. Its unification semantics are attractive for declarative intent, and its ability to express constraints is stronger than ordinary JSON Schema. 

CUE

The case for CUE is compelling: fewer languages, less glue, more consistent constraint composition, and the possibility of describing both desired state and valid outcomes in one artifact.

Why I still prefer JSON Schema + CEL initially:
- JSON is a broadly interoperable interchange representation.
- JSON Schema can remain the stable structural contract.
- CEL separates structural validation from behavioral acceptance.
- Consumers can implement the envelope without adopting a specialized constraint language.
- The system can later support CUE as an authoring or validation profile.
CUE is the alternative I would prototype before freezing v1.0. I would not rule it out based on theoretical preference alone.

TM Forum is the strongest intent-model alternative, but its telecom orientation and licensing questions make it a weaker default for general-purpose agent work.
5. Is design by contract the right framing?
Yes, but it is insufficient by itself.

Traditional design by contract asks whether an operation satisfies preconditions, postconditions and invariants. Here the system must also establish that the result is complete, grounded in fixed evidence, and acceptable under an explicit equivalence relation.

The more precise formulation is:
\[
\operatorname{Accept}(R,I,C,V)=\mathrm{true}
\]

where \(R\) is the result, \(I\) the pinned inputs, \(C\) the contract and \(V\) the pinned verifier.

Two executions are equivalent when:
\[
R_1\sim_C R_2
\]

This does not require identical output bytes. For example, two lists with different ordering may be equivalent if the contract declares order irrelevant.
Changes to the proposed framing
Original element	Recommendation
Target state	Keep; allow analytical artifacts as outcomes, not only state changes
Pinned inputs	Keep; require content digests
Exact deliverable	Keep; add canonicalization
Acceptance checks	Keep; define verifier semantics
Boundaries and budget	Keep; distinguish hard limits from accounting
Ambiguity rules	Keep; reject unresolved semantic ambiguity
Failure meanings	Keep; distinguish incomplete from incorrect
Evidence	Keep; add completeness evidence
Authority and risk	Keep, but externalize authorization verification
Stable identity	Keep; distinguish request ID from content hash
Invariants	Merge into acceptance and safety constraints
Equivalence	Add explicitly
Verifier version	Add explicitly
Input observability	Add explicitly
Side-effect/commit policy	Add for mutating work

Deterministic acceptance is achievable for bounded, formally checkable tasks. Deterministic truth is not generally achievable for open-world research, subjective judgments, incomplete observations or undecidable program properties.

For those tasks, the contract should support INDETERMINATE, not fabricate certainty.
6. Worked example: PaymentService rename analysis
The example below is illustrative, not an actual repository scan. The abbreviated commit is insufficient to pin a repository uniquely; a real request must resolve it to a full commit ID before execution.
Request

contract_version: "0.1"
request_id: "rename-impact-001"

intent:
  kind: "code.impact_analysis"
  outcome: "Enumerate every impacted source location"
  change:
    symbol: "PaymentService"
    rename_to: "PaymentGateway"

inputs:
  repository: "payments-service"
  commit: "4f2c9e1"
  resolution: "require_unique_full_commit"
  source_tree_digest: "<required-sha256>"

scope:
  include: ["**/*"]
  exclude: [".git/**"]
  analysis: "static"
  writes_allowed: false
  network_allowed: false

ambiguity:
  symbol_resolution: "reject_if_multiple"
  dynamic_references: "report_as_unresolved"

output:
  schema: "rename-impact-result-v1"
  order_by: ["file", "line", "column"]

acceptance:
  - id: "valid_structure"
    verifier: "json-schema-2020-12"
  - id: "valid_locations"
    verifier: "source-location-v1"
  - id: "complete_coverage"
    verifier: "reference-coverage-v1"
  - id: "all_evidence_pinned"
    verifier: "source-digest-v1"

failure_policy:
  partial_allowed: true
  success_requires_all_checks: true
  unresolved_means: "PARTIAL"

evidence_policy:
  file_and_line_required: true
  completeness_required: true



The verifier names above are proposed project-defined identifiers, not published standard verifiers. Production requests must also pin their implementation versions and digests.
Illustrative result: partial failure

{
  "contract_version": "0.1",
  "request_id": "rename-impact-001",
  "status": "PARTIAL",
  "artifact": {
    "references": [
      {
        "file": "src/api/Checkout.java",
        "line": 42,
        "column": 18,
        "kind": "type_reference",
        "evidence": "ev-001"
      },
      {
        "file": "src/config/Payments.java",
        "line": 17,
        "column": 9,
        "kind": "constructor_reference",
        "evidence": "ev-002"
      }
    ],
    "unresolved": [
      {
        "file": "src/runtime/Loader.java",
        "line": 88,
        "reason": "dynamic_symbol_lookup"
      }
    ]
  },
  "checks": [
    {"id": "valid_structure", "status": "PASS"},
    {"id": "valid_locations", "status": "PASS"},
    {"id": "complete_coverage", "status": "FAIL"},
    {"id": "all_evidence_pinned", "status": "PASS"}
  ],
  "evidence": [
    {"id": "ev-001", "source": "Checkout.java:42"},
    {"id": "ev-002", "source": "Payments.java:17"}
  ],
  "assumptions": [],
  "failure": {
    "code": "INCOMPLETE_COVERAGE",
    "details": "Dynamic symbol lookup cannot be resolved"
  },
  "usage": {
    "cost_usd": null,
    "cost_status": "NOT_RECORDED"
  }
}



The sample references are fictional. A production result must contain actual immutable source references and digests; the example's abbreviated evidence is not sufficient for independent verification.

The essential rule is that PARTIAL is not success. Valid references may be returned, but the system must not claim exhaustive coverage.

For this particular request, the harder issue is defining breakage. A symbol reference, a compilation failure, a reflective lookup, and a configuration string are different phenomena. A reliable contract must define which constitute an impacted location and what proof of exhaustiveness is acceptable.
7. Unverified claims and confidence
Finding	Confidence
No surveyed standard covers the entire contract	High within surveyed field; not an exhaustive proof
JSON Schema supports structural validation	High
CEL can support portable acceptance expressions	High for language semantics; medium for cross-runtime consistency
W3C PROV represents provenance	High
Provenance alone proves factual correctness	False
CUE is a credible single-language alternative	High
TM Forum intent models generalize cleanly to all agent tasks	Low
Complete static rename-impact analysis is always decidable	False
Apache compatibility of every candidate and derivative	Not fully verified
Model reliability following the proposed contract	Unverified; requires empirical testing

I could not verify full text and licensing conditions for every telecom document, every vendor-specific method, or all implementation-level conformance guarantees. In particular, repository adoption counts, cross-vendor model adherence, and independent CEL/CUE implementation consistency have not been benchmarked.

The coverage matrix is a qualitative analysis, not a formal standards conformance test.
8. Sources
All sources below are primary publisher, standards-body, or official project sources. No secondary source is needed for the central recommendation.
Primary source	Relevance
JSON Schema	Structural contracts
CEL specification	Acceptance expressions
CUE specification	Constraint-language alternative
OPA / Rego	Policy and assertions
W3C PROV-DM	Provenance data model
in-toto	Evidence attestations
ISO 29148	Requirements engineering
Gherkin	Executable acceptance scenarios
OpenAPI and Arazzo	API contracts and workflows
TM Forum intent standards	Intent semantics and reporting
3GPP TS 28.312	Intent management
A2A specification	Task/result states
RFC 9457	Failure representation
Spec Kit	Specification-driven development
Oracle Agent Spec	Agent/flow description
Agent Skills	Capability description
Structured outputs	Constrained model output

Final recommendation
Do not standardize an agentic prompt. Standardize a verifiable intent contract that can be authored from natural language, reviewed by a person, validated by software, and evaluated independently of whichever agent produced the result.

Before implementing the full specification, run a focused interoperability experiment with JSON Schema + CEL versus CUE, using identical requests and verifiers across multiple runtimes.

The acceptance criterion for that experiment should be agreement on validation and failure outcomes, not whether agents produce identical text.

That is the key distinction between making agent outputs look deterministic and establishing a contract under which their results can actually be trusted.




## 1. Survey of the Field

No single standard defines a complete request/result contract for non-deterministic AI agents. The field is fragmented across software testing, data validation, and semantic web standards.

* **JSON Schema:** Defines the exact structure and data types of JSON documents. *Covers: exact deliverable structure, input types, limited machine-evaluable checks.* ([Link](https://json-schema.org/))
* **Behavior-Driven Development (BDD) / Gherkin:** A domain-specific language for defining executable specifications and acceptance criteria. *Covers: machine-checkable acceptance, ambiguity handling (via explicit scenarios), target states.* ([Link](https://cucumber.io/docs/gherkin/reference/))
* **Design by Contract (DbC - Eiffel):** Software design approach utilizing preconditions, postconditions, and invariants. *Covers: boundaries, failure meanings, target states.* ([Link](https://www.eiffel.com/values/design-by-contract/))
* **W3C PROV:** An ontology for provenance, defining entities, activities, and people involved in producing a piece of data. *Covers: evidence, inputs, tracing.* ([Link](https://www.w3.org/TR/prov-overview/))
* **TM Forum Intent (TR299 / TMF921):** Telecommunications standards for intent-based networking, focusing on "what" rather than "how." *Covers: target state formulation, boundaries, request identity.* ([Link](https://www.google.com/search?q=https://www.tmforum.org/resources/reference/tr299-intent-classification-v1-1-0/))
* **DSPy Signatures:** A declarative framework for defining language model inputs/outputs and execution behavior. *Covers: inputs, output structure (partially), intent.* ([Link](https://github.com/stanfordnlp/dspy))
* **EARS (Easy Approach to Requirements Syntax):** Natural language templates for structuring system requirements. *Covers: outcome formulation, ambiguity reduction.* ([Link](https://www.ieee.org/))

## 2. Coverage Matrix

| Contract Element | JSON Schema | Gherkin | DbC (Concept) | W3C PROV | TM Forum Intent | DSPy Signatures |
| --- | --- | --- | --- | --- | --- | --- |
| **Target state / Intent** | Not covered | Partly | Fully | Not covered | Fully | Partly |
| **Pinned inputs** | Not covered | Partly | Not covered | Fully | Not covered | Partly |
| **Exact deliverable structure** | Fully | Not covered | Not covered | Not covered | Not covered | Partly |
| **Machine-checkable acceptance** | Partly | Fully | Fully | Not covered | Partly | Not covered |
| **Boundaries / Budget** | Not covered | Not covered | Partly | Not covered | Partly | Not covered |
| **Ambiguity rules** | Not covered | Partly | Not covered | Not covered | Not covered | Not covered |
| **Failure meanings** | Partly | Fully | Fully | Not covered | Partly | Not covered |
| **Evidence / Provenance** | Not covered | Not covered | Not covered | Fully | Not covered | Not covered |
| **Authority / Risk class** | Not covered | Not covered | Not covered | Partly | Not covered | Not covered |
| **Stable request identity** | Not covered | Not covered | Not covered | Fully | Fully | Not covered |

## 3. Decision

**Compose several standards.** Do not adopt one, as none cover the full agentic lifecycle. Wait on monolithic agent standards (like Oracle's or KIRO), as they lack maturity, governance, and broad independent adoption.

Create a lightweight JSON wrapper (the "Agent Contract") that composes three mature, Apache-compatible, widely adopted standards:

1. **JSON Schema** to enforce the exact deliverable structure and closed failure states.
2. **Gherkin** embedded as strings to define machine-evaluable acceptance checks and ambiguity rules.
3. **W3C PROV** concepts mapped to JSON for evidence and provenance tracing.

**Minimum to define yourself:** The JSON envelope structure itself (keys for `budget`, `boundaries`, `authority`, `request_id`, and `assumptions_made`), and the execution engine that runs the Gherkin tests against the JSON Schema output before returning the result to the user.

## 4. The Strongest Case Against This Decision

**The case for DSPy Signatures:** You should adopt DSPy. LLMs are fundamentally different from traditional software; trying to force them into rigid software paradigms like BDD/Gherkin creates brittle systems. DSPy treats prompts as compiled code and focuses on optimizing the weights and prompts against a metric. It is natively built for the probabilistic nature of LLMs, is open-source, and has massive momentum in the AI engineering community. It elegantly handles input/output mappings without forcing the user to write exhaustive test cases.

**Why the decision stands:** DSPy optimizes for *average accuracy over a dataset*, not for *strict contractual determinism on a single run*. If an enterprise wants a guaranteed contract (e.g., "Do not mutate this database, prove you read this file, output exactly this schema or fail safely"), DSPy's declarative signatures lack the strict bounding, deterministic acceptance gating, and provenance required by your fixed constraints.

## 5. The Framing

Design by Contract is the right theoretical model, but it requires a paradigm shift for AI: **Contractual Determinism vs. Generative Determinism.** You cannot guarantee the agent will take the exact same path or generate the exact same tokens. You *can* guarantee that the result returned to the user strictly adheres to the schema and passes all acceptance criteria, or it returns a defined failure.

**Critique of your list:**

* *Remove:* "Rules for handling ambiguity, decided in advance." This is too broad to machine-evaluate natively. Merge this into *Machine-evaluable acceptance checks* (e.g., if ambiguity arises, a Gherkin test dictates the default assumption).
* *Merge:* *Authority* and *Risk class* belong inside *Boundaries*.
* *Add:* **Side-effect constraints.** (e.g., "Read-only mode," "No network calls outside `github.com`"). This is critical for agent sandboxing at the contract level.
* *Add:* **Execution Trace / Reasoning Graph.** A result must include the chain of tools used, not just the final evidence, to satisfy the provenance requirement.

## 6. Example

**The Request Contract (JSON Envelope)**

```json
{
  "request_id": "req_9948a_2026",
  "intent": "Identify all breakage points if PaymentService is renamed to PaymentGateway.",
  "inputs": {
    "repository": "https://github.com/org/payments-service",
    "commit": "4f2c9e1"
  },
  "boundaries": {
    "network": ["github.com/org/payments-service"],
    "side_effects": "read-only",
    "budget": {"max_tokens": 50000, "max_tool_calls": 20}
  },
  "deliverable_schema": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "type": "array",
    "items": {
      "type": "object",
      "properties": {
        "file": {"type": "string"},
        "line_number": {"type": "integer"},
        "current_code": {"type": "string"}
      },
      "required": ["file", "line_number", "current_code"]
    }
  },
  "acceptance_checks": [
    "Given the result array, Then every 'file' must exist in commit 4f2c9e1",
    "Given the result array, Then 'current_code' must contain the string 'PaymentService'"
  ],
  "ambiguity_rules": "If a file contains 'PaymentService' in a commented-out line, ignore it."
}

```

**The Result Contract (JSON Envelope)**

```json
{
  "request_id": "req_9948a_2026",
  "status": "PARTIAL_SUCCESS",
  "deliverable": [
    {
      "file": "src/controllers/checkout.ts",
      "line_number": 42,
      "current_code": "const svc = new PaymentService();"
    }
  ],
  "acceptance_results": {
    "files_exist": "PASS",
    "contains_string": "PASS"
  },
  "assumptions_made": [
    "Ignored src/legacy/old_payment.ts line 12 due to being a comment (per ambiguity rule)."
  ],
  "evidence": [
    {"source": "src/controllers/checkout.ts@4f2c9e1", "tool_used": "github_read_file"}
  ],
  "partial_failure_reason": "Budget limit exceeded (max_tokens 50000 reached). Searched 85% of repository.",
  "cost": {"tokens_used": 50000, "tool_calls": 18}
}

```

## 7. Unverified Claims and Confidence

* **High Confidence:** JSON Schema and Gherkin are mature, Apache-compatible (or equivalent permissive licenses), and heavily adopted. They perfectly bridge human readability and machine evaluation.
* **Medium Confidence:** The applicability of TM Forum TR299/TMF921 to generic software agents. These standards are robust for telecom network intents but may be too domain-specific or heavyweight to map cleanly to general-purpose LLM agent tasks without significant abstraction.
* **Unverified:** The licensing and independent adoption metrics of specific emerging vendor standards like Oracle Open Agent Specification or Kiro specs. Many "agent standards" introduced in 2024–2026 are heavily biased toward the initiating vendor's ecosystem and lack true independent governance bodies (like W3C or IETF).

## 8. Sources

1. **JSON Schema Specification** (Primary) - [https://json-schema.org/specification](https://json-schema.org/specification)
2. **Cucumber / Gherkin Syntax** (Primary) - [https://cucumber.io/docs/gherkin/reference/](https://cucumber.io/docs/gherkin/reference/)
3. **W3C PROV Overview** (Primary) - [https://www.w3.org/TR/prov-overview/](https://www.w3.org/TR/prov-overview/)
4. **Design by Contract, Eiffel Software** (Secondary) - [https://www.eiffel.com/values/design-by-contract/](https://www.eiffel.com/values/design-by-contract/)
5. **TM Forum TR299 Intent Classification** (Primary) - [https://www.tmforum.org/resources/reference/tr299-intent-classification-v1-1-0/](https://www.google.com/search?q=https://www.tmforum.org/resources/reference/tr299-intent-classification-v1-1-0/)
6. **DSPy: Compiling Declarative Language Model Calls into Self-Improving Pipelines** (Primary) - [https://github.com/stanfordnlp/dspy](https://github.com/stanfordnlp/dspy)
