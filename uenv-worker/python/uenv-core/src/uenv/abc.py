from typing import Generic, TypeVar, Optional

ActT = TypeVar("ActT")
ObsT = TypeVar("ObsT")
StateT = TypeVar("StateT")


class Environment(Generic[ActT, ObsT, StateT]):
    def reset(self, seed: Optional[int] = None) -> ObsT:
        raise NotImplementedError

    def step(self, action: ActT) -> tuple[ObsT, float, bool, bool, dict]:
        raise NotImplementedError

    def close(self):
        raise NotImplementedError

    @property
    def state(self) -> StateT:
        raise NotImplementedError
