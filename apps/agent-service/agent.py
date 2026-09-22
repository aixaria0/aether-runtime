import os
import httpx
from langgraph.graph import StateGraph, START, END
from typing import TypedDict
from litellm import acompletion

class AgentState(TypedDict, total=False):
    goal: str
    plan: str
    result: str
    verified: bool

async def planner(state: AgentState):
    response = await acompletion(
        model=os.getenv("MODEL_NAME", "openai/gpt-4o-mini"),
        messages=[{"role": "user", "content": "Choose exactly one operation: echo, uppercase, or sha256 for this input. Reply with only the operation. Input: " + state["goal"]}],
    )
    choice = (response.choices[0].message.content or "").strip().lower()
    return {"plan": choice if choice in {"echo", "uppercase", "sha256"} else "echo"}

async def executor(state: AgentState):
    async with httpx.AsyncClient(timeout=15) as client:
        response = await client.post(os.getenv("ENGINE_API", "http://localhost:8080") + "/execute",
            json={"operation": state["plan"], "payload": state["goal"]})
        response.raise_for_status()
        return {"result": response.json()["output"]}

def verifier(state: AgentState):
    expected = state["goal"].upper() if state["plan"] == "uppercase" else state["goal"] if state["plan"] == "echo" else None
    return {"verified": bool(state["result"]) if expected is None else state["result"] == expected}

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
