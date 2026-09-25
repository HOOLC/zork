// Actual zork CLI + Station, native enrollment and the Worker's native relay without Google.
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
try {
  const workerPort = await port();
  h = await harness({
    origin: `http://127.0.0.1:${workerPort}`,
    port: workerPort,
    noGoogle: true,
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
  await cli("status", "--json");
  const config = JSON.parse(await fs.readFile(path.join(nodeRoot, "config.json"), "utf8"));
  for (const key of ["station", "runtime", "control", "agent"]) config.bind[key] = `127.0.0.1:${await port()}`;
  config.admin = { token: "isolated-native-enrollment" };
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
  await until(async () => {
    try {
      const response = await fetch("http://" + config.bind.runtime + "/v1/mesh", {
        headers: { authorization: "Bearer isolated-native-enrollment" },
      });
      return response.ok && typeof ((await response.json()) as any).origin === "string";
    } catch {
      return false;
    }
  }, "local Station readiness without Google login");
  pass("local Station is ready without an account or Google configuration");
  const budgets: any = await h.mf.getDurableObjectNamespace("RELAY_HUB");
  const budget = budgets.get(budgets.idFromName("primary"));
  await until(async () => (await budget.authenticated()) > 0, "native iroh relay handshakes without credentials");
  pass("Station data and invitation endpoints complete native iroh relay handshakes anonymously");
  assert.equal(station.pid, pid);
  assert.equal(station.exitCode, null);
  await assert.rejects(fs.access(path.join(nodeRoot, "account/relay.json")));
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
        google: "not configured; no credentials issued",
        relay: "native RelayHub Durable Object (iroh-relay-v2 protocol) in Miniflare",
        fixture: success ? "removed" : root,
      },
      null,
      2,
    ) + "\n",
  );
  if (success) await fs.rm(root, { recursive: true, force: true });
}
