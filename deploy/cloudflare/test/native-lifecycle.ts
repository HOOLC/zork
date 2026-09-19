// Actual zork CLI + Station, production core lifecycle and official relay.
// Only the external Google identity provider is replaced by a local RS256 fixture.
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import net from "node:net";
import { spawn, execFile as execCallback } from "node:child_process";
import { promisify } from "node:util";
import { createHash } from "node:crypto";
import { ulid } from "ulid";
import { harness } from "./harness.ts";
const exec = promisify(execCallback);
const [binDir, reportDir] = process.argv.slice(2);
if (!binDir || !reportDir) throw new Error("usage: native-lifecycle.ts BIN_DIR REPORT_DIR");
const binary = path.resolve(binDir, "zork"),
  stationBinary = path.resolve(binDir, "zork-station");
const report = path.resolve(reportDir);
await fs.mkdir(report, { recursive: true });
const root = await fs.mkdtemp(path.join(report, "fixture-"));
await fs.chmod(root, 0o700);
const processes: Array<ReturnType<typeof spawn>> = [];
const container = "zork-account-" + ulid().toLowerCase();
let h: Awaited<ReturnType<typeof harness>> | undefined;
let success = false;
const checks: string[] = [];
async function port() {
  const server = net.createServer();
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const value = (server.address() as net.AddressInfo).port;
  await new Promise<void>((resolve) => server.close(() => resolve()));
  return value;
}
async function until(check: () => Promise<boolean>, label: string, timeout = 30000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if (await check()) return;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error("timeout: " + label);
}
function pass(label: string) {
  checks.push(label);
  console.log("PASS: " + label);
}
async function credentials() {
  return JSON.parse(await fs.readFile(path.join(root, "node/account/relay.json"), "utf8"));
}
try {
  const relayPort = await port(),
    workerPort = await port();
  await exec("docker", ["run", "--rm", "-d", "--platform", "linux/amd64", "--name", container, "-p", `127.0.0.1:${relayPort}:8080`, "zork-relay-account-lifecycle:local"]);
  h = await harness({
    relay: `http://127.0.0.1:${relayPort}`,
    origin: `http://127.0.0.1:${workerPort}`,
    port: workerPort,
  });
  const nodeRoot = path.join(root, "node");
  await fs.mkdir(nodeRoot);
  const services = path.join(root, "services.json");
  await fs.writeFile(services, JSON.stringify({ relay_urls: [h.origin], discovery_url: h.origin + "/pkarr" }));
  const env = {
    ...process.env,
    ZORK_SERVICES_CONFIG: services,
    ZORK_MESH_LOCAL_DISCOVERY: "0",
    ZORK_MESH_LAN_DISCOVERY: "0",
    RUST_LOG: "warn",
  };
  async function cli(...args: string[]) {
    try {
      return await exec(binary, ["account", ...args, "--data", nodeRoot], { env, timeout: 30000 });
    } catch (error: any) {
      return {
        stdout: String(error.stdout ?? ""),
        stderr: String(error.stderr ?? ""),
        code: error.code ?? 1,
      };
    }
  }
  async function login() {
    const child = spawn(binary, ["account", "login", "--no-browser", "--data", nodeRoot], {
      env,
      stdio: ["ignore", "pipe", "pipe"],
    });
    processes.push(child);
    let output = "";
    child.stdout!.on("data", (chunk) => {
      output += chunk.toString();
    });
    child.stderr!.resume();
    await until(async () => output.includes("\n"), "CLI login listener");
    const start = new URL(output.split("\n")[0]);
    assert.equal(start.origin, h!.origin);
    // A forged local callback must not complete/cancel the real login.
    const local = new URL(start.searchParams.get("redirect_uri")!);
    local.searchParams.set("state", "forged");
    local.searchParams.set("code", "forged");
    assert.equal((await fetch(local)).status, 400);
    const response = await h!.fetch(start.pathname + start.search, { redirect: "manual" });
    assert.equal(response.status, 302);
    const google = new URL(response.headers.get("location")!);
    const code = ulid();
    h!.codes.set(code, {
      nonce: google.searchParams.get("nonce")!,
      challenge: google.searchParams.get("code_challenge")!,
      sub: "native-google-user",
    });
    const callback = await h!.fetch(
      "/v1/auth/google/callback?" +
        new URLSearchParams({
          state: google.searchParams.get("state")!,
          code,
        }),
      {
        redirect: "manual",
        headers: { cookie: response.headers.get("set-cookie")!.split(";")[0] },
      },
    );
    assert.equal(callback.status, 302);
    const finish = await fetch(callback.headers.get("location")!, {
      signal: AbortSignal.timeout(15000),
    });
    assert.equal(finish.status, 200);
    await until(async () => child.exitCode !== null, "CLI login completion");
    assert.equal(child.exitCode, 0);
    return (await credentials()).current;
  }
  await cli("status", "--json");
  const config = JSON.parse(await fs.readFile(path.join(nodeRoot, "config.json"), "utf8"));
  for (const key of ["station", "runtime", "control", "agent"]) config.bind[key] = `127.0.0.1:${await port()}`;
  config.mesh = {
    ...config.mesh,
    enabled: true,
    offline: false,
    bind: "127.0.0.1:0",
    relay_urls: [h.origin],
    discovery_url: h.origin + "/pkarr",
    peers: [],
    workspaces: [],
  };
  await fs.writeFile(path.join(nodeRoot, "config.json"), JSON.stringify(config));
  const log = await fs.open(path.join(root, "station.log"), "w", 0o600);
  const station = spawn(stationBinary, ["--data", nodeRoot], {
    env,
    stdio: ["ignore", log.fd, log.fd],
  });
  processes.push(station);
  const pid = station.pid;
  await h.fetch("/__test/offline");
  await until(async () => {
    try {
      const response = await fetch("http://" + config.bind.runtime + "/v1/mesh");
      return response.ok && typeof ((await response.json()) as any).origin === "string";
    } catch {
      return false;
    }
  }, "local Station readiness without Google login");
  pass("local Station is ready with no account and an unavailable login service");
  await h.fetch("/__test/offline");
  let session = await login();
  assert.equal((await fs.stat(path.join(nodeRoot, "account/relay.json"))).mode & 0o777, 0o600);
  pass("real CLI OAuth callback/PKCE exchange and private credential storage");
  async function connected(access: string) {
    if (station.exitCode !== null) throw new Error("Station exited; retained fixture log");
    const res = await h!.fetch("/v1/auth/session", {
      headers: { authorization: "Bearer " + access },
    });
    return res.status === 200 && Number(((await res.json()) as any).relay_connections) >= 2;
  }
  await until(() => connected(session.token), "Station authenticated through Worker to official relay");
  const accounts: any = await h.mf.getDurableObjectNamespace("ACCOUNTS");
  const account = accounts.get(accounts.idFromName(session.subject));
  await until(async () => Number((await account.statistics()).quota?.bytes) > 128, "native iroh challenge handshake traffic");
  pass("real Station data and invitation endpoints reach official iroh relay through authenticated Worker");
  const old = session;
  const refresh = await cli("refresh", "--json");
  assert.equal("code" in refresh, false);
  session = (await credentials()).current;
  assert.equal(session.refresh_token !== old.refresh_token, true);
  assert.equal(session.expires_at >= old.expires_at, true);
  await until(() => connected(session.token), "renewed Station relay connection");
  assert.equal(station.pid, pid);
  const concurrent = await Promise.all([cli("refresh", "--json"), cli("refresh", "--json")]);
  assert.equal(
    concurrent.every((result) => !("code" in result)),
    true,
  );
  session = (await credentials()).current;
  assert.equal((await h.fetch("/v1/auth/session", { headers: { authorization: "Bearer " + session.token } })).status, 200);
  pass("refresh and concurrent CLI rotations hot-apply without restarting Station");

  const beforeAutomatic = session;
  console.log("Waiting for the real 5-minute credential's renewal deadline...");
  await until(async () => (await credentials()).current?.refresh_token !== beforeAutomatic.refresh_token, "automatic renewal at production expiry", 260000);
  session = (await credentials()).current;
  await until(() => connected(session.token), "automatically renewed relay admission");
  assert.equal(station.pid, pid);
  pass("production-duration automatic renewal keeps the existing Station usable");

  await h.fetch("/__test/offline");
  const offlineLogout = await cli("logout", "--json");
  assert.equal("code" in offlineLogout, true, "offline logout must report remote revocation pending");
  await until(async () => (await credentials()).current === null, "local credentials disabled");
  assert.equal((await credentials()).pending_revocations.length, 1);
  await h.fetch("/__test/offline");
  await until(async () => (await credentials()).pending_revocations.length === 0, "queued logout revocation");
  assert.equal((await h.fetch("/v1/auth/session", { headers: { authorization: "Bearer " + session.token } })).status, 401);
  pass("offline logout disables locally, then retries server revocation on recovery");
  session = await login();
  await until(() => connected(session.token), "login hot-applies to existing Station");
  await h.fetch("/__test/lose-refresh-response");
  const lostRefresh = await cli("refresh");
  assert.equal("code" in lostRefresh, true, "injected lost rotation response must reach the real client");
  const logout = await cli("logout", "--all");
  assert.equal("code" in logout, false);
  assert.equal(
    (
      await h.fetch("/relay", {
        headers: { upgrade: "websocket", authorization: "Bearer " + session.token },
      })
    ).status,
    401,
  );
  assert.equal(station.pid, pid);
  pass("relogin, lost rotation response recovery and logout-all invalidate credentials without Station restart");
  await log.close();
  success = true;
} finally {
  for (const child of processes) {
    if (child.exitCode === null) {
      child.kill("SIGTERM");
      const deadline = Date.now() + 15000;
      while (child.exitCode === null && child.signalCode === null && Date.now() < deadline) await new Promise((resolve) => setTimeout(resolve, 100));
      if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL");
    }
  }
  await h?.close();
  await exec("docker", ["rm", "-f", container]).catch(() => {});
  const hashes = Object.fromEntries(
    await Promise.all(
      [binary, stationBinary].map(async (file) => [
        path.basename(file),
        createHash("sha256")
          .update(await fs.readFile(file))
          .digest("hex"),
      ]),
    ),
  );
  await fs.writeFile(
    path.join(report, "result.json"),
    JSON.stringify(
      {
        passed: success,
        checks,
        binaries: hashes,
        google: "local RS256 provider fixture; real Google configuration still required",
        relay: "official iroh-relay 1.1.0 Docker image",
        fixture: success ? "removed" : root,
      },
      null,
      2,
    ) + "\n",
  );
  if (success) await fs.rm(root, { recursive: true, force: true });
}
