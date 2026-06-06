# uenv-adapter-core

Rust adapter core for the VeRL pre-rollout Python shim.

Boundary:

```text
VeRL UEnvAgentLoop
  prompt ids / sampling params / reward config
        |
        | local gRPC: adapter_core.proto
        v
Rust adapter core
  SampleEnvelope validation / conversion / dispatch
        |
        | Rust function call
        v
EpisodeService
        |
        v
UEnv Server / Worker
```

The core does not depend on VeRL Python objects. Python sends normalized
`SampleEnvelope` messages and receives `SampleResult` messages. Server-side
code only needs to implement `EpisodeService` and return `EpisodeResult`
values that include response token ids, response mask, trajectory and reward.
