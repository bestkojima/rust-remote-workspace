"""Sample one running workspace and its child processes during a named operation."""
import asyncio
from contextlib import asynccontextmanager
import time

import psutil


@asynccontextmanager
async def measure_working_set(pid, phase):
    proc = psutil.Process(pid)
    app_peak = child_peak = samples = child_samples = 0
    started = time.monotonic()

    async def sample():
        nonlocal app_peak, child_peak, samples, child_samples
        while proc.is_running():
            try:
                rss = proc.memory_info().rss
                children = proc.children(recursive=True)
                sizes = []
                for child in children:
                    try:
                        sizes.append(child.memory_info().rss)
                    except psutil.NoSuchProcess:
                        continue
            except psutil.NoSuchProcess:
                break
            app_peak = max(app_peak, rss)
            samples += 1
            if sizes:
                child_peak = max(child_peak, sum(sizes))
                child_samples += 1
            await asyncio.sleep(.02)

    task = asyncio.create_task(sample())
    try:
        yield
    finally:
        task.cancel()
        await asyncio.gather(task, return_exceptions=True)
        child_result = f"{child_peak / 2**20:.1f} MiB" if child_samples else "not observed"
        print(f"{phase}: workspace peak {app_peak / 2**20:.1f} MiB; child tree peak {child_result}; "
              f"{samples} samples ({child_samples} with children) in {time.monotonic()-started:.2f}s")
