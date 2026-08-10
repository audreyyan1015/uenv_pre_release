"""Compatibility import for the legacy nested AdapterCore module path.

The canonical generated module lives at ``uenv.bridge.gen.adapter_core_pb2``.
Re-exporting it keeps old imports on the same protobuf descriptors and message
classes instead of registering a second, stale copy of the protocol.
"""

from ...adapter_core_pb2 import *  # noqa: F403
from ...adapter_core_pb2 import DESCRIPTOR as DESCRIPTOR
