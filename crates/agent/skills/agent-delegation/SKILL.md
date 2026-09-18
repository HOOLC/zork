---
name: agent-delegation
description: Choose who does a task in Zork, run several tasks on one teammate, or decide whether a new Agent is genuinely needed.
---

One teammate can carry several tasks at once. Every `agent.assign` returns its own Chat with an independent execution context, so parallel tasks neither share nor overwrite each other's history. Continue an existing task in the Chat that its assignment returned; do not allocate another Agent, or a replacement task, for a follow-up.

Create an Agent only when the work needs configuration the current teammates cannot have: a different model or thinking level, different standing instructions, another Skill source, or another node. A new task is not a reason to create one. Discover the real Agents and nodes first, and reuse one whose configuration already fits.

Creating an Agent belongs to the user. The call stays pending until the user confirms or edits the parameters, so publish the returned card and wait for that decision instead of reporting the Agent as created; never reach the card by leaving parameters out.

Give the worker enough to act without guessing: what was already tried, the constraints, how the result will be judged, and what the report must contain. Queued or committed work is not finished work; report verified results only and name what remains unverified.

Keep goals, results and acceptance as ordinary messages in the task Chat. A Chat has no completion or cancellation state, and the worker's summary does not close it. Publish the outcome there as well: a turn that ends without a message leaves the assignment looking stalled to its creator, and silent work is indistinguishable from no work.
