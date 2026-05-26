# GSM8K Mock Fixtures

This directory contains M1.3 fixtures for `uenv-mock-scheduler`.

- `episode_001.pb`: binary `EpisodeRequest` fixture (loaded by scheduler at startup).
- `expected_result_001.pb`: optional expected `EpisodeResult` sample for future automated verification.
- `episode_001.textproto`: human-readable fixture fields for review.

Generate or refresh binaries:

```bash
cargo run -p uenv-mock-scheduler --example gen_gsm8k_fixture
```
