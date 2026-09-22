import asyncio
import os
from datetime import timedelta
from temporalio import workflow, activity
from temporalio.client import Client
from temporalio.worker import Worker

@activity.defn
async def run_agent(goal: str) -> dict:
    from agent import run
    return await run(goal)

@workflow.defn
class ExecutionWorkflow:
    @workflow.run
    async def run(self, goal: str) -> dict:
        return await workflow.execute_activity(run_agent, goal,
            start_to_close_timeout=timedelta(seconds=90))

async def worker_main():
    client = await Client.connect(os.getenv("TEMPORAL_ADDRESS", "localhost:7233"))
    worker = Worker(client, task_queue="aether-execution", workflows=[ExecutionWorkflow], activities=[run_agent])
    await worker.run()

if __name__ == "__main__":
    asyncio.run(worker_main())
