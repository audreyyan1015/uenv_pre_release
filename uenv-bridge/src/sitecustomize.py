"""Process-wide Python startup hooks for uenv-bridge smoke tests.

This module is imported automatically by Python when it is present on
PYTHONPATH. Keep behavior behind explicit environment flags.
"""

from __future__ import annotations

import os


def _patch_resource_tracker_duplicate_unregister() -> None:
    """Tolerate duplicate shared-memory UNREGISTER messages in Python 3.12.

    vLLM's multiprocessing workers can emit duplicate resource-tracker
    unregister events during shutdown. CPython 3.12's resource tracker uses
    set.remove(), so the second unregister prints a KeyError traceback even
    after training has completed successfully. The tracker process is launched
    as a fresh Python interpreter that imports this sitecustomize module before
    running ``from multiprocessing.resource_tracker import main``.
    """

    import signal
    import sys
    import warnings
    from multiprocessing import resource_tracker as rt

    def main(fd: int) -> None:
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        if rt._HAVE_SIGMASK:
            signal.pthread_sigmask(signal.SIG_UNBLOCK, rt._IGNORED_SIGNALS)

        for stream in (sys.stdin, sys.stdout):
            try:
                stream.close()
            except Exception:
                pass

        cache = {rtype: set() for rtype in rt._CLEANUP_FUNCS.keys()}
        try:
            with open(fd, "rb") as file:
                for line in file:
                    try:
                        cmd, name, rtype = line.strip().decode("ascii").split(":")
                        cleanup_func = rt._CLEANUP_FUNCS.get(rtype)
                        if cleanup_func is None:
                            raise ValueError(
                                f"Cannot register {name} for automatic cleanup: "
                                f"unknown resource type {rtype}"
                            )

                        if cmd == "REGISTER":
                            cache[rtype].add(name)
                        elif cmd == "UNREGISTER":
                            cache[rtype].discard(name)
                        elif cmd == "PROBE":
                            pass
                        else:
                            raise RuntimeError(f"unrecognized command {cmd!r}")
                    except Exception:
                        try:
                            sys.excepthook(*sys.exc_info())
                        except Exception:
                            pass
        finally:
            for rtype, rtype_cache in cache.items():
                if rtype_cache:
                    try:
                        warnings.warn(
                            "resource_tracker: There appear to be %d leaked %s "
                            "objects to clean up at shutdown"
                            % (len(rtype_cache), rtype)
                        )
                    except Exception:
                        pass
                for name in rtype_cache:
                    try:
                        rt._CLEANUP_FUNCS[rtype](name)
                    except Exception as exc:
                        warnings.warn(f"resource_tracker: {name!r}: {exc}")

    rt.main = main


if os.environ.get("UENV_PATCH_RESOURCE_TRACKER") == "1":
    _patch_resource_tracker_duplicate_unregister()


def _patch_verl_agent_loop_batch() -> None:
    from uenv.bridge.verl_batch_agent_loop_patch import apply_verl_agent_loop_batch_patch

    apply_verl_agent_loop_batch_patch()


if os.environ.get("UENV_AGENT_LOOP_BATCH", "0").strip().lower() in {"1", "true", "yes", "on"}:
    _patch_verl_agent_loop_batch()
