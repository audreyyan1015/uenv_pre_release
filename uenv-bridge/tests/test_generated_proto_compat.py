from __future__ import annotations

import unittest


try:
    import google.protobuf  # noqa: F401
except ModuleNotFoundError:
    HAS_PROTOBUF = False
else:
    HAS_PROTOBUF = True


@unittest.skipUnless(HAS_PROTOBUF, "protobuf dependency is not installed")
class GeneratedProtoCompatibilityTest(unittest.TestCase):
    def test_legacy_module_reexports_canonical_messages(self) -> None:
        from uenv.bridge.gen import adapter_core_pb2 as canonical
        from uenv.bridge.gen.uenv.v1 import adapter_core_pb2 as legacy

        self.assertIs(legacy.DESCRIPTOR, canonical.DESCRIPTOR)
        self.assertIs(legacy.ExecuteBatchRequest, canonical.ExecuteBatchRequest)


if __name__ == "__main__":
    unittest.main()
