#!/usr/bin/python3
import json
import pathlib
import sys

mode = "FIXTURE_MODE"
log = pathlib.Path(__file__).with_name("requests.jsonl")
turn = 0
sessions = 0
stalled = False
def emit(value):
    print(json.dumps(value), flush=True)

for line in sys.stdin:
    with log.open("a") as stream:
        stream.write(line)
    message = json.loads(line)
    method = message.get("method")
    if "id" not in message:
        continue
    if method == "initialize":
        result = {"userAgent": "synthetic"}
    elif method in ("thread/start", "thread/resume"):
        sessions += 1
        result = {"thread": {"id": "invented-thread", "sessionId": "invented-session"}, "model": "actual-hidden", "cwd": "/tmp/invented-project", "reasoningEffort": "extra_native"}
        if mode == "many-sessions":
            result["thread"]["id"] = message["params"].get("threadId", f"thread-{sessions}")
        if mode == "missing-model":
            del result["model"]
    elif method == "model/list":
        if mode == "stall-once" and not stalled:
            stalled = True
            continue
        if not message["params"].get("cursor") or mode == "repeat-cursor":
            result = {"data": [{"model": "wrong-default", "isDefault": True}], "nextCursor": "page-two"}
        else:
            result = {"data": [{"model": "actual-hidden", "hidden": True, "defaultReasoningEffort": "medium", "supportedReasoningEfforts": [{"reasoningEffort": level} for level in ["low", "medium", "extra_native"]]}], "nextCursor": None}
            if mode == "unknown-model":
                result["data"] = []
            if mode == "oversized-levels":
                result["data"][0]["supportedReasoningEfforts"] = [{"reasoningEffort": f"level{i}"} for i in range(33)]
    elif method == "config/read":
        result = {} if mode == "missing-config" else {"config": {"model_reasoning_effort": "invalid" if mode == "invalid-default" else "low", "private_ignored": "must-not-be-retained"}}
        if mode == "null-config":
            result["config"]["model_reasoning_effort"] = None
        if mode == "unset-config":
            del result["config"]["model_reasoning_effort"]
    elif method == "turn/start":
        turn += 1
        result = {"turn": {"id": f"turn-{turn}"}}
    else:
        result = {}
    emit({"id": message["id"], "result": result})
    if method == "turn/start":
        if mode == "changed-model":
            emit({"method": "thread/settings/updated", "params": {"threadId": "invented-thread", "threadSettings": {"model": "new-unknown-model", "cwd": "/tmp/invented-project", "effort": "extra_native"}}})
        emit({"method": "turn/completed", "params": {"threadId": message["params"]["threadId"], "turn": {"id": f"turn-{turn}", "status": "completed"}}})
