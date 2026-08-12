#!/usr/bin/env python3
"""Time CogniNucleus FreeEnergyAgent vs Lumia triad-forward shape (CPU).

Not bit-identical to `bench_cn_forward.lm` — that is a dense-float skeleton.
This measures real PyTorch agent `forward` / `forward+learn` at the same
vis/pfc/mot dims with EFE on and hip/amy off (closest triad match).
"""

from __future__ import annotations

import argparse
import os
import statistics
import sys
from time import perf_counter

import torch

# CogniNucleus repo root (sibling of Lumia by default).
CN_ROOT = os.environ.get(
    "COGNINUCLEUS_ROOT",
    os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", "CogniNucleus")),
)
sys.path.insert(0, CN_ROOT)

from cogninucleus.agent import FreeEnergyAgent  # noqa: E402
from cogninucleus.config import AgentConfig  # noqa: E402


def make_agent() -> FreeEnergyAgent:
    cfg = AgentConfig(
        vis_size=16,
        pfc_size=32,
        mot_size=4,
        enable_hippocampus=False,
        enable_amygdala=False,
        enable_efe=True,
        efe_horizon=2,
        efe_pref_gain=1.2,
        efe_epi_gain=0.28,
        efe_wall_gain=3.2,
        efe_goal_bonus=0.8,
        efe_threat_gain=2.0,
        efe_mix=0.35,
        enable_prune=False,
        enable_grow=False,
        enable_async=False,
        enable_precision=False,
        add_shortcut_vis_motor=False,
        enable_event_async=False,
        use_goal_memory=False,
        wall_avoid_gain=0.0,
        motor_prior_gain=0.0,
        state_lr=0.1,
        conn_lr=0.05,
        mu_clip=10.0,
        weight_clip=5.0,
        weight_decay=1e-4,
    )
    agent = FreeEnergyAgent(cfg)
    agent.eval()
    return agent


def bump_obs(obs: torch.Tensor, t: int) -> None:
    obs.zero_()
    obs[0] = 0.5
    obs[1] = 0.5
    obs[4] = 0.25 - (t % 20) * 0.01
    obs[11] = 0.1


def time_loop(steps: int, learn: bool, warmup: int) -> float:
    agent = make_agent()
    obs = torch.zeros(16)
    for t in range(warmup):
        bump_obs(obs, t)
        agent.forward(obs, explore=False)
        if learn:
            agent.learn(0.0)
    bump_obs(obs, 0)
    t0 = perf_counter()
    for t in range(steps):
        bump_obs(obs, t)
        agent.forward(obs, explore=False)
        if learn:
            agent.learn(0.0)
    return perf_counter() - t0


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("--steps", type=int, default=20000)
    p.add_argument("--warmup", type=int, default=200)
    p.add_argument("--runs", type=int, default=3)
    args = p.parse_args()

    torch.set_num_threads(int(os.environ.get("TORCH_NUM_THREADS", "1")))
    # Intra-op parallelism on tiny matmuls hurts; keep deterministic single-thread.
    if hasattr(torch, "set_num_interop_threads"):
        try:
            torch.set_num_interop_threads(1)
        except RuntimeError:
            pass

    print(f"torch {torch.__version__}  threads={torch.get_num_threads()}")
    print(f"steps={args.steps} warmup={args.warmup} runs={args.runs}")
    print("config: triad 16/32/4  EFE horizon=2  hip/amy off  prune/grow/async off")

    for label, learn in (("forward", False), ("forward+learn", True)):
        samples = []
        for _ in range(args.runs):
            samples.append(time_loop(args.steps, learn=learn, warmup=args.warmup))
        med = statistics.median(samples)
        us = med / args.steps * 1e6
        print(
            f"torch_{label}  time(s)  min/med/max  "
            f"{min(samples):.4f}  {med:.4f}  {max(samples):.4f}  "
            f"({us:.1f} µs/step med)"
        )
        print(f"TORCH_{label.upper().replace('+', '_').replace('-', '_')}_US={us:.1f}")


if __name__ == "__main__":
    main()
