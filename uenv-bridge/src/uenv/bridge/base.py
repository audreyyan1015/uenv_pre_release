from __future__ import annotations

from typing import Any


class BaseAdapter:
    def convert_request(self, request: Any) -> Any:
        raise NotImplementedError

    def convert_response(self, response: Any) -> Any:
        raise NotImplementedError
