import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";

import { afterEach, describe, expect, it } from "vite-plus/test";

import { brokerRoot, getFreePort, removeTempRoot, spawnBinary, stopChild, waitFor, waitForReady, writeConfig } from "./helpers.js";
import { MockSlackServer } from "./helpers/mock-slack-server.js";

const agentToken = "merged-mailbox-token";
const connectionId = "01J00000000000000000000MRG";

function slackConnection(slackPort: number): Record<string, unknown> {
  return {
    id: connectionId,
    name: "Merged Test Slack",
    provider: "slack",
    enabled: true,
    mode: "normal",
    app_token: "xapp-test",
    bot_token: "xoxb-test",
    api_base_url: `http://127.0.0.1:${slackPort}/api`,
  };
}

function sessionKey(channelId: string, rootMessageId: string): string {
  return `${connectionId}:${channelId}:${rootMessageId}`;
}

describe.sequential("Station and Agent mailbox integration", () => {
  const cleanups: Array<() => Promise<void>> = [];

  afterEach(async () => {
    while (cleanups.length > 0) {
      await cleanups.pop()?.();
    }
  });

  it("routes Slack input to Agent without projecting assistant transcript", { timeout: 60_000 }, async () => {
    const tempRoot = await fs.mkdtemp(path.join(os.tmpdir(), "merged-mailbox-"));
    cleanups.push(async () => removeTempRoot(tempRoot));
    const stateDir = path.join(tempRoot, "state");
    await writeFakeProfile(tempRoot);

    const slack = new MockSlackServer("UBOT");
    const slackPort = await slack.start();
    cleanups.push(async () => slack.stop());

    const stationPort = await getFreePort();
    const runtimePort = await getFreePort();
    const controlPort = await getFreePort();
    const agentPort = await getFreePort();
    await writeConfig(tempRoot, {
      im_connections: [slackConnection(slackPort)],
      bind: {
        station: `127.0.0.1:${stationPort}`,
        runtime: `127.0.0.1:${runtimePort}`,
        control: `127.0.0.1:${controlPort}`,
        agent: `127.0.0.1:${agentPort}`,
      },
    });

    const station = spawnBinary("zork-station", {
      cwd: brokerRoot,
      args: ["--data", tempRoot, "--fake-agent", "--agent-token", agentToken],
    });
    cleanups.push(async () => stopChild(station));

    const agentBase = `http://127.0.0.1:${agentPort}`;
    await waitForReady(`${agentBase}/readyz`, "Agent readyz");
    await waitForReady(`http://127.0.0.1:${runtimePort}/readyz`, "Station broker readyz");
    await waitForReady(`http://127.0.0.1:${stationPort}/readyz`, "Station Slack readyz");
    await slack.waitForSocket();

    await slack.sendEvent("evt-merged-1", {
      type: "app_mention",
      user: "U123",
      channel: "C123",
      thread_ts: "100.200",
      ts: "100.201",
      text: "<@UBOT> hello mailbox",
    });

    await waitFor(
      () => readInboundMessages(stateDir, sessionKey("C123", "100.200")),
      (rows) => rows.some((row) => row.message_ts === "100.201" && row.status === "delivered"),
      "mailbox receipt",
    );
    const identity = readSessionIdentity(stateDir, sessionKey("C123", "100.200"));
    const bot = await fetch(`http://127.0.0.1:${stationPort}/sessions/${encodeURIComponent(sessionKey("C123", "100.200"))}/im/bot`);
    expect(bot.status).toBe(200);
    await expect(bot.json()).resolves.toMatchObject({ ok: true, self: { userId: "UBOT" } });
    const workspace = path.join(tempRoot, "shared-files", "workspaces", "im", connectionId, "normal", "C123", "100.200");
    expect(identity.workspace_path).toBe(workspace);
    await waitFor(
      async () => readAgentStatus(agentBase, identity.id),
      (status) => status === "finished",
      "Agent session finished",
    );
    expect(slack.postedMessages).toHaveLength(0);

    await waitFor(
      () => slack.assistantStatusUpdates,
      (updates) => updates.some((update) => update.channel === "C123" && update.status === "Thinking...") && updates.some((update) => update.channel === "C123" && update.status === ""),
      "Agent status projected to Slack",
    );

    const waitResponse = await fetch(`${agentBase}/sessions/${identity.id}/mailbox`, {
      method: "POST",
      headers: { authorization: `Bearer ${agentToken}`, "content-type": "application/json" },
      body: JSON.stringify({
        content: JSON.stringify({
          fake_tool: {
            name: "wait",
            input: { reason: "the controlled build", seconds: 12 },
          },
        }),
      }),
    });
    expect(waitResponse.status).toBe(202);
    // The wait control supplies the earliest batch deadline; the status uses
    // the scripted invocation action, while its countdown still refreshes every five seconds.
    const waitPrefix = "Waiting: 调用 wait to finish · ";
    const waitUpdates = await waitFor(
      () => slack.assistantStatusUpdates,
      (updates) => updates.filter((update) => update.channel === "C123" && update.status.startsWith(waitPrefix)).length >= 2,
      "five-second Slack wait countdown",
    ).catch((error: Error) => {
      throw new Error(`${error.message}: ${JSON.stringify(slack.assistantStatusUpdates)}`);
    });
    const countdown = waitUpdates.filter((update) => update.channel === "C123" && update.status.startsWith(waitPrefix));
    expect(countdown[1]!.atMs - countdown[0]!.atMs).toBeGreaterThanOrEqual(4_500);
    expect(countdown[1]!.atMs - countdown[0]!.atMs).toBeLessThan(6_500);
    expect(countdown[0]!.status).not.toBe(countdown[1]!.status);

    const removedStatusApi = await fetch(`http://127.0.0.1:${stationPort}/sessions/${encodeURIComponent(sessionKey("C123", "100.200"))}/im/threads/C123/100.200/status`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ status: "must not be accepted" }),
    });
    expect(removedStatusApi.status).toBe(404);

    const removedStateApi = await fetch(`http://127.0.0.1:${runtimePort}/chat/post-state`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({}),
    });
    expect(removedStateApi.status).toBe(404);

    const files = await fs.readdir(stateDir);
    expect(files.some((name) => name.startsWith("spool.sqlite"))).toBe(false);
    expect(files).toContain("station.sqlite");
    expect(files).not.toContain("runtime.sqlite");
    expect(files).not.toContain("control.sqlite");
    const stationDb = new DatabaseSync(path.join(stateDir, "station.sqlite"), { readOnly: true });
    const stationTables = stationDb
      .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name IN ('admin_operations', 'admin_audit_events') ORDER BY name")
      .all()
      .map((row) => String(row.name));
    stationDb.close();
    expect(stationTables).toEqual(["admin_audit_events", "admin_operations"]);

    const reset = await fetch(`http://127.0.0.1:${runtimePort}/sessions/${encodeURIComponent(sessionKey("C123", "100.200"))}/reset`, {
      method: "POST",
    });
    expect(reset.status).toBe(200);
    expect(readInboundMessages(stateDir, sessionKey("C123", "100.200")).some((row) => row.source === "admin_session_reset")).toBe(false);
    const resetIdentity = readSessionIdentity(stateDir, sessionKey("C123", "100.200"));
    expect(resetIdentity.id).not.toBe(identity.id);
    expect(resetIdentity.workspace_path).toBe(workspace);
    expect(slack.postedMessages.some((message) => message.text.includes("admin_session_reset"))).toBe(false);

    // Deletion now uses the embedded Agent; removing the canonical runtime
    // session first must still allow idempotent Station binding cleanup.
    const directDelete = await fetch(`${agentBase}/sessions/${resetIdentity.id}`, {
      method: "DELETE",
      headers: { authorization: `Bearer ${agentToken}` },
    });
    expect(directDelete.status).toBe(204);
    const deleted = await fetch(`http://127.0.0.1:${runtimePort}/sessions/${encodeURIComponent(sessionKey("C123", "100.200"))}`, {
      method: "DELETE",
    });
    expect(deleted.status).toBe(200);
    expect(readOptionalSessionIdentity(stateDir, sessionKey("C123", "100.200"))).toBeUndefined();
    expect((await fs.stat(workspace)).isDirectory()).toBe(true);
  });

  it("deduplicates a Slack redelivery across Station restart before appending a second mailbox message", { timeout: 60_000 }, async () => {
    const tempRoot = await fs.mkdtemp(path.join(os.tmpdir(), "merged-mailbox-replay-"));
    cleanups.push(async () => removeTempRoot(tempRoot));
    const stateDir = path.join(tempRoot, "state");
    await writeFakeProfile(tempRoot);

    const slack = new MockSlackServer("UBOT");
    const slackPort = await slack.start();
    cleanups.push(async () => slack.stop());
    const stationPort = await getFreePort();
    const runtimePort = await getFreePort();
    const controlPort = await getFreePort();
    const agentPort = await getFreePort();
    await writeConfig(tempRoot, {
      im_connections: [slackConnection(slackPort)],
      bind: {
        station: `127.0.0.1:${stationPort}`,
        runtime: `127.0.0.1:${runtimePort}`,
        control: `127.0.0.1:${controlPort}`,
        agent: `127.0.0.1:${agentPort}`,
      },
    });

    const firstStation = spawnBinary("zork-station", {
      cwd: brokerRoot,
      args: ["--data", tempRoot, "--fake-agent", "--agent-token", agentToken],
    });
    await waitForReady(`http://127.0.0.1:${runtimePort}/readyz`, "first Station readyz");
    await slack.waitForSocket();
    await slack.sendEvent("evt-replay-1", {
      type: "app_mention",
      user: "U123",
      channel: "C223",
      thread_ts: "200.200",
      ts: "200.201",
      text: "<@UBOT> replay me",
    });
    await waitFor(
      () => readInboundMessages(stateDir, sessionKey("C223", "200.200")),
      (rows) => rows.some((row) => row.message_ts === "200.201" && row.status === "delivered"),
      "first mailbox receipt",
    );
    const identity = readSessionIdentity(stateDir, sessionKey("C223", "200.200"));
    firstStation.kill("SIGKILL");
    await new Promise<void>((resolve) => firstStation.once("exit", () => resolve()));

    const secondStation = spawnBinary("zork-station", {
      cwd: brokerRoot,
      args: ["--data", tempRoot, "--fake-agent", "--agent-token", agentToken],
    });
    cleanups.push(async () => stopChild(secondStation));
    await waitForReady(`http://127.0.0.1:${runtimePort}/readyz`, "second Station readyz");
    await slack.waitForSocket();
    await slack.sendEvent("evt-replay-2", {
      type: "app_mention",
      user: "U123",
      channel: "C223",
      thread_ts: "200.200",
      ts: "200.201",
      text: "<@UBOT> replay me",
    });
    await waitFor(
      () => slack.acknowledgedEnvelopeIds,
      (ids) => ids.includes("env-evt-replay-2"),
      "redelivered Slack envelope mailbox receipt",
    );
    expect(readInboundMessages(stateDir, sessionKey("C223", "200.200")).filter((row) => row.message_ts === "200.201")).toHaveLength(1);
    const agentMessages = (await fetch(`http://127.0.0.1:${agentPort}/sessions/${identity.id}/messages`, {
      headers: { authorization: `Bearer ${agentToken}` },
    }).then((response) => response.json())) as { items?: Array<{ type?: string; role?: string; content?: string }> };
    expect(agentMessages.items?.filter((message) => message.role === "mailbox" && message.content?.includes("replay me"))).toHaveLength(1);
    expect(slack.postedMessages).toHaveLength(0);
  });
});

async function writeFakeProfile(dataRoot: string): Promise<void> {
  const profileDir = path.join(dataRoot, "profiles");
  await fs.mkdir(profileDir, { recursive: true });
  await fs.writeFile(
    path.join(profileDir, "test.json"),
    JSON.stringify({
      provider: "xai",
      billing: "subscription",
      models: [
        {
          id: "grok-4.6",
          api: "openai-completions",
          streaming: true,
          thinking: ["off"],
          default_thinking: "off",
          capabilities: { input: ["text"] },
          limits: { context_window_tokens: 131_072, max_output_tokens: 8_192 },
          default: true,
        },
      ],
      auth: { type: "api_key", key: "test-key" },
    }),
  );
}

async function readAgentStatus(baseUrl: string, sessionId: string): Promise<string> {
  const response = await fetch(`${baseUrl}/sessions/${sessionId}`, {
    headers: { authorization: `Bearer ${agentToken}` },
  });
  if (!response.ok) return "";
  const body = (await response.json()) as { status?: string };
  return String(body.status || "");
}

function readSessionIdentity(stateDir: string, key: string): { id: string; workspace_path: string } {
  const session = readOptionalSessionIdentity(stateDir, key);
  if (!session) throw new Error(`missing session ${key}`);
  return session;
}

function readOptionalSessionIdentity(stateDir: string, key: string): { id: string; workspace_path: string } | undefined {
  const db = new DatabaseSync(path.join(stateDir, "station.sqlite"), { readOnly: true });
  try {
    return db.prepare("SELECT id, workspace_path FROM sessions WHERE key = ?").get(key) as { id: string; workspace_path: string } | undefined;
  } finally {
    db.close();
  }
}

function readInboundMessages(stateDir: string, sessionKey: string): Array<{ message_ts: string; source: string; status: string }> {
  const db = new DatabaseSync(path.join(stateDir, "station.sqlite"), { readOnly: true });
  try {
    return db.prepare("SELECT message_ts, source, status FROM inbound_messages WHERE session_key = ? ORDER BY created_at, message_ts").all(sessionKey) as Array<{ message_ts: string; source: string; status: string }>;
  } finally {
    db.close();
  }
}
