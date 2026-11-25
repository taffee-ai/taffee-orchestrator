# Research brief: which published specs should an agent orchestrator adopt?

You are acting as a standards analyst for an open-source (Apache-2.0) agent platform. Recommend which published specifications the platform should adopt for each layer described below, and justify each choice. Take a clear position for every layer: adopt, adopt with a documented profile or extension, watch, or reject. "It depends" is not an acceptable answer; when the evidence is close, choose and say what would change your mind.

Verify everything against primary sources (the spec text, the official repository, the standards body's page). Several candidates are new or moving fast, so check the current version, licence, governance and real adoption yourself, and cite a link for every factual claim. If you find a better candidate that is not listed, include it.

## Context: the system the specs must fit

**Taffee** is an open-source platform for running enterprise AI agents. Its pillars are context (a code and knowledge graph served over MCP), a capabilities registry (versioned, evidence-qualified tools, agents and models), governance (authorization decisions in an AuthZEN shape, short-lived scoped credentials, delegation chains that can only narrow), and the **orchestrator**, which is the subject of this brief.

**The orchestrator today:**
- It is one Rust process on Wasmtime 49. Each agent is a separate WebAssembly component instance: a memory sandbox with its own 64 MB limit, not an OS process or thread. Agents share a small async thread pool, and about 300 resident agents fit in 1.4 GB.
- The reference agent is an unmodified **AWS Strands Agents** TypeScript SDK agent, compiled to a WASI 0.2 component. The guest contract is a single export, `run-turn(json) -> json`.
- The agent never holds credentials. Every outbound HTTP call (model or tool) passes through a host gate that checks the destination against the agent's allow list, injects credentials (AWS SigV4 or Bearer), and records the call.
- An agent is created from a **spec**: model, system prompt, tools, MCP servers, limits (model turns, tokens, tool-output size) and seed history.

**The planned design (what the specs must express):**
1. **Intent in.** A request arrives as an intent: an objective, the requester, constraints, acceptance criteria and a risk class.
2. **Outer plan, done once and heavy.** A strong model turns the intent into a plan: steps, and for each step a worker brief (the worker's main prompt), the named toolsets and skills it gets, a model tier, limits and acceptance criteria. The plan also pre-authorizes classes of follow-up work.
3. **Inner loop, light and repeated.** Each worker is a separate Wasm agent running a fast model with a narrow toolset. It completes its brief and returns a structured result.
4. **Workers propose and the controller decides.** Workers never start agents, send messages that have side effects, or wait on their own. They end with an outcome (done, partial, blocked or needs input, not found, plus proposed follow-ups). Host-side code then decides: start follow-ups inside the plan's allowance, send the rest back to the outer planner, refuse, ask a human, or wait.
5. **Enforceable prompts, our intended innovation.** The host sits between the agent and everything it touches, so it can make parts of a prompt binding. A rule like "MUST NOT write" becomes a toolset grant, "MUST only call X" becomes the egress allow list, "MUST finish within N steps" becomes a turn limit, and "MUST cite sources" becomes a post-run check. Each rule in a brief is classified as host-enforced, checked after the run, or advisory, and the run record says which was which.

**Constraints on the choices:**
- They must work with the Strands Agents SDK running unmodified inside Wasm. Configuring the SDK is fine; patching it is not.
- They must be usable under Apache-2.0, with no field-of-use or patent restrictions.
- They should not require any one cloud vendor.
- A plain-text format a human can review in a pull request is preferred.

## The layers to decide

For each layer, the candidates we know of are listed. Check them, and add others.

### Layer 1: Intent (what outcome is wanted, and how fulfilment is reported)
- **TM Forum TR299 Intent Specification and the TMF921 Intent Management API.** Intent is expectations (requirements, goals, constraints), with set, report and negotiate operations.
  - https://tmforum.org/resources/standard/tr299-intent-specification-v3-6-0
  - https://tmforum.org/resources/standard/tmf921-intent-management-api-user-guide-v5-0-0
  - https://www.tmforum.org/resources/standard/tmf921a-intent-management-api-profile-v1-1-0/
  - https://tmforum.org/resources/standard/ig1253-intent-in-autonomous-networks-v1-3-0
- **3GPP TS 28.312**, intent-driven management: https://www.3gpp.org/DynaReport/28312.htm
- Questions to answer:
  - Can a telecom intent model be used for agent tasks without importing network-specific baggage?
  - Are TM Forum documents licensed for open-source reuse?
  - Is there an agent-native alternative?

### Layer 2: Worker brief or procedure (the main prompt the outer planner writes for each worker)
- **Strands Agent SOPs** (`.sop.md`: parameters, steps, RFC 2119 MUST/SHOULD/MAY keywords, served as MCP prompts)
  - https://github.com/strands-agents/agent-sop
  - https://aws.amazon.com/blogs/opensource/introducing-strands-agent-sops-natural-language-workflows-for-ai-agents
  - https://strandsagents.com/blog/introducing-strands-agent-sops/
- **MCP prompts**, the protocol's prompt-template primitive: https://modelcontextprotocol.io/specification
- **Prompt file formats.**
  - POML: https://github.com/microsoft/poml, https://arxiv.org/abs/2508.13948
  - Prompty: https://github.com/microsoft/prompty
  - Dotprompt: https://github.com/google/dotprompt
- Questions to answer:
  - Can the format carry machine-readable rule strength (MUST, SHOULD, MAY) so a runtime can classify and enforce rules?
  - Can parameters be bound safely, keeping user text as data and not as instructions?
  - Can an LLM generate the format reliably?

### Layer 3: Packaged know-how and capability grants
- **Agent Skills (`SKILL.md`)**: name, description and allowed-tools frontmatter, scripts and references, loaded only when relevant.
  - https://agentskills.io/
  - https://agentskills.io/specification
  - https://github.com/agentskills/agentskills
- **AGENTS.md**, repository-level instructions for agents: https://agents.md
- Questions to answer:
  - Can `allowed-tools` (or an equivalent field) be treated as a binding grant that a host enforces?
  - How stable is the core field set?

### Layer 4: Agent and workflow definition (what an agent is, stored in a registry)
- **Oracle Open Agent Specification**: agents and flows, JSON or YAML, runtime adapters.
  - https://arxiv.org/abs/2510.04173
  - https://github.com/oracle/agent-spec
- **Upstream Strands WIT `strands:agent@0.1.0`**, `agent-config`: https://github.com/strands-agents/sdk-typescript (see `wit/agent.wit`)
- **AGNTCY OASF**, the Open Agentic Schema Framework: https://github.com/agntcy/oasf
- **Workflow specifications.**
  - OpenAPI Arazzo: https://spec.openapis.org/arazzo/latest.html
  - CNCF Serverless Workflow: https://serverlessworkflow.io
- Questions to answer:
  - Is any of these settled enough to be the registry's stored format, or should we keep our own spec and offer an import or export?

### Layer 5: Task exchange and worker outcomes (states, artifacts, agent-to-agent hand-off)
- **A2A (Agent2Agent)**: task states `submitted`, `working`, `input-required`, `auth-required`, `completed`, `failed`, `canceled` and `rejected`; artifacts; agent cards. Note that v1.0 renamed the states.
  - https://github.com/a2aproject/A2A
  - https://a2a-protocol.org
- **MCP elicitation and sampling**, for asking a human and for asking the client's model: https://modelcontextprotocol.io/specification
- Questions to answer:
  - Do the A2A task states cover our worker outcomes (done, partial, blocked/needs-input, needs-auth, not-found, failed, timed out, crashed)?
  - Which outcomes need an extension, and how should one be added without breaking conformance?
  - What changed between 0.3 and 1.0, and which should we target?

### Also check
Anything else that defines intent-to-agent prompting, agent task outcomes, or enforceable agent policies: for example Google ADK agent config, OpenAI Agents SDK configuration, IBM's ACP (reportedly merged into A2A), AuthZEN for the authorization side (https://openid.net/wg/authzen/), and any work on making prompt rules machine-enforceable.

## Evaluation criteria (apply to every candidate)
1. **Fit**: does it express what the layer needs, as described above?
2. **Maturity and governance**: version, standards body or foundation, change process, stability of the core.
3. **Adoption**: independent implementations, not only the authoring vendor.
4. **Licence**: is it clearly usable in Apache-2.0 software and docs?
5. **Strands and Wasm compatibility**: does it work with the Strands TS SDK unmodified, inside a WASI component?
6. **Enforceability**: can a runtime turn parts of it into checks rather than leaving it as text?
7. **Exit cost**: how hard would it be to switch later?

## Deliverable

Return one markdown document with these sections.

1. **Summary.** One table: layer, chosen spec and version, stance (adopt, adopt with profile, watch, reject), and a one-line reason.
2. **Per layer:**
   - **Decision and rationale**, scored against the seven criteria.
   - **Field mapping:** how our concepts map to the spec's fields, and what has no home.
     - Layer 1: objective, requester, constraints, acceptance criteria, risk class.
     - Layer 2: brief, parameters, MUST/SHOULD rules, toolsets, skills, model tier, limits.
     - Layer 5: the worker outcomes listed above.
   - **The profile or extension we would need,** written as the smallest set of added fields. Show how to add it without breaking conformance.
   - **Rejected alternatives,** with the reason for each.
   - **Risks,** and what would make us revisit the decision.
3. **The enforceable-prompts idea.** Is there prior art (a standard or a product) for classifying prompt rules as runtime-enforced, post-checked or advisory? If there is, cite it. If not, say so, and propose how the chosen Layer 2 and Layer 3 formats could carry the classification: a field, a convention, or a companion file.
4. **One worked example.** An intent ("List what breaks if `PaymentService` is renamed in the payments-service repo; cite file and line for each item"), shown in the chosen Layer 1 format. Then the outer plan's brief for one worker, in the chosen Layer 2 format with a Layer 3 skill reference. Then the worker's result, in the chosen Layer 5 format, including one proposed follow-up.
5. **Sources.** Every link you used, marked primary or secondary, with the date you accessed it.

Be concrete and brief. Prefer tables and short paragraphs. Do not pad.
