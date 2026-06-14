from __future__ import annotations

import asyncio
import logging

logger = logging.getLogger(__name__)


def apply_verl_vllm_shutdown_patch() -> None:
    """Add explicit vLLM server shutdown to VeRL's Ray trainer path.

    In the current pre-rollout smoke test, UEnv Server/Worker performs the
    generation, but VeRL still starts its vLLM HTTP server for weight sync and
    log-prob computation. Without an explicit shutdown, Ray can tear down the
    vLLM actor while the EngineCore monitor is still alive, which vLLM reports
    as "Engine core proc ... died unexpectedly" after training has completed.
    """

    try:
        _patch_vllm_http_server()
        _patch_rollout_replica()
        _patch_llm_server_manager()
        _patch_ray_ppo_trainer()
    except Exception:
        logger.exception("failed to install VeRL vLLM shutdown patch")
        raise


def _patch_vllm_http_server() -> None:
    from verl.workers.rollout.vllm_rollout import vllm_async_server

    cls = vllm_async_server.vLLMHttpServer
    if getattr(cls, "_uenv_shutdown_patch_applied", False):
        return

    async def shutdown(self) -> None:
        if getattr(self, "_uenv_shutdown_started", False):
            return
        self._uenv_shutdown_started = True

        server_task = getattr(self, "_server_task", None)
        if server_task is not None and not server_task.done():
            server_task.cancel()
            try:
                await server_task
            except asyncio.CancelledError:
                pass
            except Exception:
                logger.exception("vLLM uvicorn task failed during shutdown")

        engine = getattr(self, "engine", None)
        if engine is not None:
            try:
                shutdown_fn = getattr(engine, "shutdown", None)
                if shutdown_fn is not None:
                    shutdown_fn()
            except Exception:
                logger.exception("vLLM engine shutdown failed")

        for attr in ("_master_sock", "_dp_rpc_sock", "_dp_master_sock"):
            sock = getattr(self, attr, None)
            if sock is not None:
                try:
                    sock.close()
                except Exception:
                    logger.debug("failed to close %s", attr, exc_info=True)
                setattr(self, attr, None)

    cls.shutdown = shutdown
    cls._uenv_shutdown_patch_applied = True


def _patch_rollout_replica() -> None:
    from verl.workers.rollout.replica import RolloutReplica

    if getattr(RolloutReplica, "_uenv_shutdown_patch_applied", False):
        return

    async def shutdown(self) -> None:
        if getattr(self, "_uenv_shutdown_started", False):
            return
        self._uenv_shutdown_started = True

        servers = list(getattr(self, "servers", []) or [])
        if servers:
            import ray

            await asyncio.gather(
                *[server.shutdown.remote() for server in servers],
                return_exceptions=True,
            )
            for server in servers:
                try:
                    ray.kill(server, no_restart=True)
                except Exception:
                    logger.debug("failed to kill rollout server actor", exc_info=True)

    RolloutReplica.shutdown = shutdown
    RolloutReplica._uenv_shutdown_patch_applied = True


def _patch_llm_server_manager() -> None:
    from verl.utils.ray_utils import auto_await
    from verl.workers.rollout.llm_server import LLMServerManager

    if getattr(LLMServerManager, "_uenv_shutdown_patch_applied", False):
        return

    @auto_await
    async def shutdown(self) -> None:
        replicas = list(getattr(self, "rollout_replicas", []) or [])
        if replicas:
            await asyncio.gather(
                *[replica.shutdown() for replica in replicas],
                return_exceptions=True,
            )

        load_balancer = getattr(self, "global_load_balancer", None)
        if load_balancer is not None:
            import ray

            try:
                ray.kill(load_balancer, no_restart=True)
            except Exception:
                logger.debug("failed to kill rollout load balancer actor", exc_info=True)

    LLMServerManager.shutdown = shutdown
    LLMServerManager._uenv_shutdown_patch_applied = True


def _patch_ray_ppo_trainer() -> None:
    from verl.trainer.ppo.ray_trainer import RayPPOTrainer

    if getattr(RayPPOTrainer, "_uenv_shutdown_patch_applied", False):
        return

    original_fit = RayPPOTrainer.fit

    def fit(self, *args, **kwargs):
        try:
            return original_fit(self, *args, **kwargs)
        finally:
            manager = getattr(self, "llm_server_manager", None)
            if manager is not None and hasattr(manager, "shutdown"):
                try:
                    manager.shutdown()
                except Exception:
                    logger.exception("VeRL LLM server manager shutdown failed")

    RayPPOTrainer.fit = fit
    RayPPOTrainer._uenv_shutdown_patch_applied = True
