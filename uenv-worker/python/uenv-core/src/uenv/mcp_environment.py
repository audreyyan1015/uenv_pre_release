from .abc import Environment


class MCPEnvironment(Environment):
    def _step_impl(self, action):
        raise NotImplementedError
