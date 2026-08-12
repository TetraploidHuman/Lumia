#!/usr/bin/env python3
"""Time CogniNucleus FreeEnergyAgent on the same triad+grid shape as Lumia port."""

from __future__ import annotations

import argparse
import os
import statistics
import sys
from time import perf_counter

import torch

CN_ROOT = os.environ.get(
    "COGNINUCLEUS_ROOT",
    os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", "CogniNucleus")),
)
sys.path.insert(0, CN_ROOT)

from cogninucleus.agent import FreeEnergyAgent  # noqa: E402
from cogninucleus.config import AgentConfig, EnvConfig  # noqa: E402
from cogninucleus.env import GridWorld  # noqa: E402


def make_agent() -> FreeEnergyAgent:
    cfg = AgentConfig(
        vis_size=16,
        pfc_size=32,
        mot_size=4,
        enable_hippocampus=False,
        enable_amygdala=False,
        enable_hypothalamus=False,
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
        strict_pe=True,
        enable_cluster_rates=True,
        nucleus_gen_lr=0.002,
    )
    agent = FreeEnergyAgent(cfg)
    agent.eval()
    return agent


def run_episodes(episodes: int, max_steps: int) -> tuple[float, int, int]:
    env = GridWorld(
        EnvConfig(grid_size=5, max_steps=max_steps, n_obstacles=0, n_threats=0),
        seed=0,
    )
    total_steps = 0
    act_sum = 0
    t0 = perf_counter()
    for _ in range(episodes):
        # Fresh agent each episode (matches Lumia bench_episodes.lm).
        agent = make_agent()
        env.reset()
        env.agent_pos = (0, 0)
        env.goal_pos = (4, 4)
        env._prev_dist = 8.0
        obs = env._observe()
        done = False
        steps = 0
        while not done and steps < max_steps:
            action = agent.forward(obs, explore=False)
            obs, reward, done = env.step(int(action))
            agent.learn(float(reward))
            act_sum += int(action)
            steps += 1
            total_steps += 1
    return perf_counter() - t0, total_steps, act_sum


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("--episodes", type=int, default=500)
    p.add_argument("--max-steps", type=int, default=50)
    p.add_argument("--runs", type=int, default=3)
    p.add_argument("--warmup", type=int, default=1)
    args = p.parse_args()

    torch.set_num_threads(int(os.environ.get("TORCH_NUM_THREADS", "1")))
    if hasattr(torch, "set_num_interop_threads"):
        try:
            torch.set_num_interop_threads(1)
        except RuntimeError:
            pass

    print(f"torch {torch.__version__}  threads={torch.get_num_threads()}")
    print(
        f"episodes={args.episodes} max_steps={args.max_steps} "
        f"runs={args.runs} warmup={args.warmup}"
    )
    print("config: triad 16/32/4 strict_pe+cluster  hip/amy/hyp off  5x5 grid")

    for _ in range(args.warmup):
        run_episodes(min(5, args.episodes), args.max_steps)

    samples: list[float] = []
    total_steps = 0
    act_sum = 0
    for _ in range(args.runs):
        dt, total_steps, act_sum = run_episodes(args.episodes, args.max_steps)
        samples.append(dt)

    med = statistics.median(samples)
    us = med / max(1, total_steps) * 1e6
    print(
        f"torch_episodes  time(s)  min/med/max  "
        f"{min(samples):.4f}  {med:.4f}  {max(samples):.4f}  "
        f"({us:.1f} µs/step med)"
    )
    print(f"TORCH_EPISODES_US={us:.1f}")
    print(f"TORCH_TOTAL_STEPS={total_steps}")
    print(f"TORCH_ACT_SUM={act_sum}")


if __name__ == "__main__":
    main()
