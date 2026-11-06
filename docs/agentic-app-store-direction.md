# Agentic App Store Direction

Recorded from the user's product direction on 2026-11-05. This is a roadmap,
not a claim that agent or RAG plugin runtimes already exist.

## Product

Grengin should become an Agentic App Store: people install trusted capabilities
for their favorite apps and use them through prompts and agents. The experience
must be native, fast and understandable, not a directory of half-working API
wrappers. Auth configuration, LLM providers and connector plugins form the current
foundation. Native agent-building and RAG-building plugins are later layers that
compose those capabilities rather than inventing separate authentication, model
and connector systems.

## Current Boundary

- Single backend instance, bounded in-process caches and per-user credentials.
  No Redis or distributed-system dependency is implied.
- Connector v1 covers approved main read-only workflows, not every vendor API.
  OAuth scopes, licensing, pagination and useful content reads matter more than
  a large tool count. Metadata-only search must not masquerade as document reading.
- Native means in-process declarative HTTP execution, not downloaded executable
  code. Prompt apps and later agent/RAG definitions must not gain arbitrary code
  execution by being labeled plugins.
- Connector v2 CRUD needs explicit action authorization, per-action confirmation,
  audit behavior and least-privilege re-consent. Never silently grant mutation
  capability to an existing v1 installation.

## Catalog Quality Contract

An app needs a coherent path from discovery to connection, search/listing, reading
an item, paging and explaining failures. No unsupported preview or paid feature
should be presented as universally available. Show exact constraints and the
signed-in account's state, with repairable expired/revoked connections.

Maintain separate evidence levels: schema validated, contract tested, official
reference reviewed, real OAuth tested and representative live tools tested. A
passing mock or reachable documentation URL must never automatically promote an
app to fully verified. Record source/version, reviewed date and missing workflows.
Keep immutable package versions, backend compatibility indexes and independent
approval for publication. Existing installs should not silently change capabilities.

Later agent/RAG packages should declare typed dependencies on compatible auth,
model and connector contracts. Permission checks and per-user credential isolation
remain enforced at invocation, not just installation. Tool results and retrieved
documents are untrusted input; they cannot grant new permissions or bypass action
confirmation. RAG must preserve source ownership, deletion and retention rules.

## Evolution

1. Finish the connector read-only audit, representative credential tests and
   usable result/pagination behavior before advertising a mature app experience.
2. Add governed write capabilities as a separately reviewed major capability.
3. Define typed native agent and RAG package schemas with bounded execution,
   dependency compatibility, cancellation, auditable operations and evaluations.
4. Compose prompt apps from these reusable layers, keeping implementation status
   and security guarantees explicit in both the catalog and documentation.

This records product intent only. It authorizes no release, automatic install,
new credential grant, migration or publication.
