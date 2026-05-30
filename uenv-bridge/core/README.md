# uenv-adapter-core

Rust adapter core for local Python shim integration.

Boundary:

```text
VeRL Python shim
  DataProto / tensors / tokenizer / rm_scores
        |
        | gRPC: adapter_core.proto
        v
Rust adapter core
  envelope validation / conversion / logs / retry
        |
        | Rust function call
        v
UEnv Server library API
```

The core should not depend on VeRL Python objects. Python sends normalized
`SampleEnvelope` messages and receives `SampleResult` messages.
