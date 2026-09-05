# Philosophy

An agent is a compute slot: a configured capacity to perform work. Applications
decide what work to assign, what context to provide, and what authority to grant.
The slot's provider and process are implementation details, subject to explicit
capability requirements.

Our model separates four concepts:

- **Slots** provide execution capacity through a configured provider.
- **Sessions** organize continuing context independently of any slot.
- **Runs** execute work on a selected slot with explicit inputs, configuration,
  and authority.
- **Records** describe messages, actions, interactions, and results, preserving
  their identity, order, and provenance.

A participant's identity is separate from the slot doing its work. A session
can use different slots across runs. Switching slots transfers only the context
we explicitly supply; it does not imply transferring private provider state.

Applications compose these concepts into experiences. Group conversations,
review pipelines, background jobs, and changing personas are application choices,
not special cases in the core. Storing a record does not automatically start work.

Persistence preserves records and execution state across process lifetimes. It
supports recovery without promising that every interrupted run can resume.
Store large resources by reference and avoid persisting every streaming delta.

Keep the core small, typed, and independent of protocols and UI frameworks.
Provide useful defaults and explicit extension points. Providers are
interchangeable where they satisfy the same required behavior; unsupported
semantics must remain visible rather than silently approximated.
