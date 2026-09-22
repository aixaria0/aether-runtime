import hashlib
import os
from typing import TypedDict

import httpx
from langgraph.graph import END, START, StateGraph
from litellm import acompletion


class AgentState(TypedDict, total=False):
    goal: str
    plan: str
    result: str
    verified: bool
    task_id: str
    output_sha256: str
    error: str


VALID_OPERATIONS = frozenset({"echo", "uppercase", "sha256"})


async def planner(state: AgentState):
    # The LLM may suggest an operation, but never gets authority to execute arbitrary code.
    response = await acompletion(
        model=os.getenv("MODEL_NAME", "openai/gpt-4o-mini"),
        messages=[{
            "role": "user",
            "content": (
                "Select exactly one operation (echo, uppercase, sha256) for the input; "
                "return ONLY the operation name.\nInput: " + state["goal"]
            ),
        }],
    )
    proposal = (response.choices[0].message.content or "").strip().lower()
    return {"plan": proposal if proposal in VALID_OPERATIONS else "echo"}


async def executor(state: AgentState):
    if state["plan"] not in VALID_OPERATIONS:
        return {"error": "invalid plan", "result": "", "verified": False}
    async with httpx.AsyncClient(timeout=15) as client:
        response = await client.post(
            os.getenv("ENGINE_API", "http://localhost:8080") + "/execute",
            json={"operation": state["plan"], "payload": state["goal"]},
        )
        response.raise_for_status()
        data = response.json()
    if not data.get("success"):
        return {"error": data.get("error", "execution failed"), "result": "", "verified": False}
    return {
        "task_id": data["task_id"],
        "result": data["output"],
        "output_sha256": data["output_sha256"],
    }


def verifier(state: AgentState):
    if state.get("error") or state.get("plan") not in VALID_OPERATIONS:
        return {"verified": False}
    goal, result, plan = state["goal"], state.get("result", ""), state["plan"]
    expected = {
        "echo": lambda: goal,
        "uppercase": lambda: goal.upper(),
        "sha256": lambda: hashlib.sha256(goal.encode()).hexdigest(),
    }[plan]()
    digest_ok = hashlib.sha256(result.encode()).hexdigest() == state.get("output_sha256")
    return {"verified": result == expected and digest_ok}


def build_graph():
    graph = StateGraph(AgentState)
    graph.add_node("planner", planner)
    graph.add_node("executor", executor)
    graph.add_node("verifier", verifier)
    graph.add_edge(START, "planner")
    graph.add_edge("planner", "executor")
    graph.add_edge("executor", "verifier")
    graph.add_edge("verifier", END)
    return graph.compile()


async def run(goal: str):
    return await build_graph().ainvoke({"goal": goal})
