"""UEnv OpenHands integration (plan §5.3.3 / §5.6).

Adapts an external OpenHands agent loop onto the UEnv Worker **External Runtime
Gateway** (L4, HTTP). Two layers:

- ``client.UEnvGatewayClient`` / ``UEnvSession``: dependency-free HTTP client
  (urllib only) for the gateway contract
  ``/runtime/v1/sessions[/{id}/{exec,read,write,submit}]``. Validated offline
  against a live Worker gateway.
- ``runtime.UEnvRuntime``: a version-agnostic, duck-typed adapter that maps
  OpenHands ``Runtime`` actions (``CmdRunAction`` / ``FileReadAction`` /
  ``FileWriteAction``) onto a ``UEnvSession``, so an OpenHands SWE-bench run can
  use UEnv as its remote runtime without UEnv depending on a specific OpenHands
  release.
"""

from .client import GatewayError, UEnvGatewayClient, UEnvSession
from .runtime import UEnvRuntime

try:
    from .workspace import UEnvWorkspace
except ImportError:
    UEnvWorkspace = None  # type: ignore

__all__ = [
    "UEnvGatewayClient",
    "UEnvSession",
    "GatewayError",
    "UEnvRuntime",
    "UEnvWorkspace",
]
