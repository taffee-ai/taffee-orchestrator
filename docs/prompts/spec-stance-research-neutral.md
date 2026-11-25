# Research brief: the request and result contract between an outside party and an agent system

You are an independent standards analyst. An open-source (Apache-2.0) project is deciding how outside parties must state the work they want from its agent system, and what they get back. The goal is an outcome that is deterministic. The project has not decided, and does not want you to confirm a preference. If our framing of the problem is wrong, say so.

Work in this order:
1. Read the problem and the constraints.
2. Survey the field yourself. List every published specification, standard, method or widely used format that addresses any part of the problem, before you look at the appendix.
3. Only then, check the appendix for anything you missed. It is alphabetical and is not a shortlist.
4. Decide.

Verify claims against primary sources (spec text, standards bodies, official repositories) and cite a link for each one. State what you could not verify and how confident you are.

## Scope

**In scope:** the content and meaning of a request and of its result. What must be stated, in what structure, with what rules, so that the outcome is predictable and checkable.

**Out of scope:** how messages are carried. Transport and RPC protocols (agent-to-agent protocols, tool-call protocols, HTTP APIs, message queues) don't matter here. Where a protocol also defines content semantics, such as task states or result structures, assess only those semantics.

**Inside the system** (planning, agents, tools, sandboxing) is entirely under the project's control and is not part of this question.

## The problem

Anyone can be the outside party: a person, a program, or another AI agent. It sends the system a request for work, and gets back a result.

The system uses language models, so the path it takes to a result can differ between runs. The project wants the **outcome** to be deterministic anyway. That means:
- the same request, with the same inputs, produces results that are equivalent under the request's own criteria;
- a result is returned only if it can be shown to meet the request;
- failure is reported in a known, closed form;
- any result can be traced to the inputs and evidence it rests on.

The question: **what must a request contain, and what must a result contain, to make that true?** And does a published specification already define it, completely or in part?

## What we believe so far (challenge it)

We suspect the answer is a contract in the design-by-contract sense: preconditions, postconditions and invariants. We think a request needs at least:
- an outcome stated as a target state, not an activity;
- pinned inputs (exact sources and versions);
- an exact deliverable structure;
- acceptance checks a machine can evaluate;
- boundaries: what may be read, used, changed or contacted, and a budget;
- rules for handling ambiguity, decided in advance;
- defined failure meanings;
- a requirement to supply evidence;
- the requester's authority and the risk class (what needs approval);
- a stable request identity, for repeat and replay.

And we think a result needs:
- a status from a closed set;
- the deliverable, in the requested structure;
- a pass or fail for each acceptance check;
- evidence for each claim;
- the assumptions made under the ambiguity rules;
- what was used and what it cost.

This list is ours. It may be missing elements, have unnecessary ones, or be the wrong model entirely.

## Fixed constraints
- Usable in Apache-2.0 software and documentation, with no field-of-use or patent restrictions.
- No dependence on a particular vendor.
- Readable and writable by both people and language models.
- Checkable by code. The parts that decide acceptance must be machine-evaluable.

## Evaluation criteria
1. **Fit.** How much of the request and result contract does it define?
2. **Determinism support.** Does it express pinned inputs, exact output structure, machine-checkable acceptance, ambiguity rules and closed failure states?
3. **Maturity and governance.** Version, owning body, change process.
4. **Independent adoption.**
5. **License.**
6. **Usable by both people and models.** Can a person write it, and can a model reliably produce and follow it?
7. **Cost of leaving.**

## What to return
1. **Survey.** Every candidate you found before reading the appendix, with which contract elements it covers, a one-line description and a link. Include requirements-engineering, testing and API-contract standards, not only AI-specific ones.
2. **Coverage matrix.** Rows: the contract elements (ours plus any you add). Columns: the candidates. Cells: fully, partly, or not covered.
3. **Decision.** Adopt one specification, compose several (say which covers which elements), write our own, or wait. Give the minimum we would have to add or define ourselves.
4. **The strongest case against your decision.** Argue for the runner-up as if you believed in it, then say why you still chose as you did.
5. **The framing.** Is design by contract the right model? Which elements in our list would you remove, merge or add? Is a deterministic outcome achievable this way, and what are its limits?
6. **Example.** A request and its result, in your chosen format, for: "List every place that would break if the class `PaymentService` were renamed to `PaymentGateway` in repository payments-service at commit 4f2c9e1; cite file and line for each." Include acceptance checks, one ambiguity rule, and one partial-failure case.
7. **Unverified claims and confidence.**
8. **Sources,** each marked primary or secondary.

Be brief. Tables and short paragraphs.

---

## Appendix: names we have come across (alphabetical; check only after your own survey)

Agent Skills (SKILL.md) · Behaviour-driven development / Gherkin · CloudEvents · Design by contract (Eiffel) · DSPy signatures · EARS (Easy Approach to Requirements Syntax) · GitHub Spec Kit (spec-driven development) · ISO/IEC/IEEE 29148 (requirements engineering) · JSON Schema · Kiro specs · OpenAPI · Oracle Open Agent Specification · Provider structured-output modes (JSON Schema–constrained outputs) · Strands Agent SOPs · Task-state models in agent protocols (content semantics only) · TM Forum TR299 / TMF921 intent · 3GPP TS 28.312 intent · W3C PROV (provenance)
