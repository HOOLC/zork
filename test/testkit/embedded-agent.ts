import fs from "node:fs/promises";
import { readdirSync, readFileSync, existsSync } from "node:fs";
import path from "node:path";
import { waitFor } from "../helpers.js";

export const embeddedAgentToken = "embedded-agent-token";
type Fact = { sessionId: string; event: Record<string, any> };

/** Observe real Agent persistence. This fixture never serves or simulates the Agent API. */
export class EmbeddedAgent {
  private root = "";
  private base = "";
  constructor(
    private readonly profiles: Array<Record<string, any>> = [
      {
        profile_id: "fixture",
        provider: "xai",
        billing: "usage",
        models: [{ id: "grok-4.6", api: "openai-completions", streaming: true, parallel_tool_calls: false, thinking: ["high", "xhigh"], default_thinking: "xhigh", capabilities: { input: ["text", "image"] }, default: true }],
      },
    ],
  ) {}

  async configure(root: string, port: number): Promise<void> {
    this.root = root;
    this.base = `http://127.0.0.1:${port}`;
    await fs.mkdir(path.join(root, "profiles"), { recursive: true });
    for (const profile of this.profiles) {
      const document = { provider: profile.provider, billing: profile.billing, auth: { type: "api_key", key: "sk-fixture" }, base_url: "http://127.0.0.1:9/v1", models: profile.models.map((model: Record<string, any>) => ({ ...model, limits: { context_window_tokens: 131072, max_output_tokens: 8192 } })) };
      await fs.writeFile(path.join(root, "profiles", `${profile.profile_id}.json`), JSON.stringify(document));
    }
  }

  private facts(): Fact[] {
    const sessions = path.join(this.root, "shared-files", "sessions");
    if (!existsSync(sessions)) return [];
    return readdirSync(sessions)
      .sort()
      .flatMap((sessionId) => {
        const segments = path.join(sessions, sessionId, "segments");
        if (!existsSync(segments)) return [];
        return readdirSync(segments)
          .filter((name) => name.endsWith(".jsonl"))
          .sort()
          .flatMap((name) =>
            readFileSync(path.join(segments, name), "utf8")
              .split("\n")
              .filter(Boolean)
              .flatMap((line) => {
                try {
                  return [{ sessionId, event: JSON.parse(line).event }];
                } catch {
                  return [];
                }
              }),
          );
      });
  }
  get creates(): Array<Record<string, any>> {
    return this.facts()
      .filter((f) => f.event.kind === "session_created")
      .map((f) => ({ context: null, ...f.event.selection, system_prompt: f.event.system_prompt, workspace: f.event.workspace }));
  }
  get appends(): Array<{ sessionId: string; body: { content: string } }> {
    return this.facts()
      .filter((f) => f.event.kind === "input_appended")
      .sort((a, b) => a.event.input.received_at_ms - b.event.input.received_at_ms || a.event.input.input_id.localeCompare(b.event.input.input_id))
      .map((f) => ({ sessionId: f.sessionId, body: { content: f.event.input.content } }));
  }
  get selectionUpdates(): Array<{ sessionId: string; selection: Record<string, unknown> }> {
    return this.facts()
      .filter((f) => f.event.kind === "selection_changed")
      .map((f) => ({ sessionId: f.sessionId, selection: f.event.selection }));
  }
  async waitForCreate(count: number) {
    return (
      await waitFor(
        () => this.creates,
        (list) => list.length >= count,
        "persisted Agent creation",
      )
    )[count - 1];
  }
  async waitForAppend(count: number) {
    return (
      await waitFor(
        () => this.appends,
        (list) => list.length >= count,
        "persisted Agent input",
      )
    )[count - 1];
  }
  async request(route: string, method = "GET", body?: unknown): Promise<Response> {
    return fetch(this.base + route, { method, headers: { authorization: `Bearer ${embeddedAgentToken}`, "content-type": "application/json" }, ...(body === undefined ? {} : { body: JSON.stringify(body) }) });
  }
}
