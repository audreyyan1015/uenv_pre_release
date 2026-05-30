# UEnvHub Error Codes (L14)

Every non-2xx response uses a single JSON envelope:

```json
{
  "error": {
    "code": "VERSION_ALREADY_EXISTS",
    "message": "version already exists: math@1.0.0",
    "details": { "kind": "version", "id": "math@1.0.0" }
  },
  "request_id": "req_abc123"
}
```

* `error.code` — stable machine-readable identifier (match on this).
* `error.message` — human-readable, may change.
* `error.details` — optional structured context.
* `request_id` — correlates with server logs and the `x-request-id` header.

## Code ↔ HTTP status

| Code | HTTP | Meaning |
|------|------|---------|
| `UNAUTHORIZED` | 401 | token missing or invalid |
| `FORBIDDEN` | 403 | not allowed (role / namespace) |
| `NOT_FOUND` | 404 | environment / version / token does not exist |
| `VERSION_ALREADY_EXISTS` | 409 | version already published (no overwrite) |
| `ENV_ALREADY_EXISTS` | 409 | environment type already exists |
| `CONFLICT` | 409 | other uniqueness / state conflict |
| `INVALID_MANIFEST` | 422 | manifest structurally invalid |
| `INVALID_VERSION` | 422 | version not valid semver |
| `INVALID_CONSTRAINT` | 422 | version constraint not parseable |
| `SCHEMA_VALIDATION_FAILED` | 422 | config / interface / default_config failed JSON Schema validation |
| `RATE_LIMITED` | 429 | per-token rate limit exceeded |
| `INTERNAL_ERROR` | 500 | unexpected server error |

## Mapping internals

`uenv-hub-core::HubError` is the single source of truth in the data/domain layer.
`uenv-hub-server::errors::ApiError` provides a **total** `From<HubError>` mapping
to the table above, plus transport-only variants (`UNAUTHORIZED`, `RATE_LIMITED`).
The client SDK decodes the envelope back into `ClientError::Api { code, .. }`, so
all three layers agree on `code`.

### Validation failures

`SCHEMA_VALIDATION_FAILED` carries a `ValidationReport` in `details`:

```json
{
  "error": {
    "code": "SCHEMA_VALIDATION_FAILED",
    "message": "schema validation failed",
    "details": {
      "valid": false,
      "issues": [
        { "severity": "error", "location": "default_config", "message": "\"x\" is a required property" }
      ]
    }
  },
  "request_id": "req_..."
}
```

The same `ValidationReport` type is produced locally by `uenv env validate`
(via `validate_manifest_local`), so the CLI and server report identical issues.
