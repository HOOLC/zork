import { expect } from "vite-plus/test";

import type { EmbeddedAgent } from "./embedded-agent.js";

export async function verifyContextConfiguration(contextUrl: string, agent: EmbeddedAgent, sessionId: string): Promise<void> {
  const context = await fetch(contextUrl);
  expect(context.status).toBe(200);
  expect(await context.json()).toEqual({ strategy: "compaction", keep_recent_tokens: 20_000 });
  const savedContext = await fetch(contextUrl, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ strategy: "handoff", keep_recent_tokens: 8_000 }),
  });
  expect(savedContext.status).toBe(200);
  expect(await savedContext.json()).toEqual({ strategy: "handoff", keep_recent_tokens: 8_000 });
  // A direct Agent-side change is visible on the next read, without a
  // Station database copy masking the authoritative setting.
  expect((await agent.request(`/sessions/${sessionId}/context`, "PUT", { strategy: "compaction", keep_recent_tokens: 4000 })).status).toBe(200);
  expect(await (await fetch(contextUrl)).json()).toEqual({ strategy: "compaction", keep_recent_tokens: 4_000 });
  const invalidContext = await fetch(contextUrl, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ strategy: "invalid", keep_recent_tokens: -1 }),
  });
  expect(invalidContext.status).toBe(422);
}
