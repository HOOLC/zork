// Real desktop widgets, Rust account controller, owned Station and official relay.
// Only Google is replaced by the same signed fixture as the Worker unit tests.
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import net from "node:net";
import { spawn, execFile as execCallback } from "node:child_process";
import { promisify } from "node:util";
import { ulid } from "ulid";
import { harness } from "./harness.ts";
const exec = promisify(execCallback);
const [testBinary, binDir, outputArg] = process.argv.slice(2);
if (!testBinary || !binDir || !outputArg) throw new Error("usage: product-account.ts HEADLESS_TEST BIN_DIR REPORT_DIR");
const output = path.resolve(outputArg);
await fs.mkdir(output, { recursive: true });
const root = await fs.mkdtemp(path.join(output, "fixture-"));
await fs.chmod(root, 0o700);
const ui = path.join(root, "ui");
await fs.mkdir(ui, { recursive: true });
const processes: ReturnType<typeof spawn>[] = [];
const container = "zork-account-product-" + ulid().toLowerCase();
let h: Awaited<ReturnType<typeof harness>> | undefined;
const checks: string[] = [];
async function port() {
  const server = net.createServer();
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const value = (server.address() as net.AddressInfo).port;
  await new Promise<void>((resolve) => server.close(() => resolve()));
  return value;
}
async function exists(file: string) {
  return fs.access(file).then(
    () => true,
    () => false,
  );
}
async function until(check: () => Promise<boolean>, label: string, timeout = 90000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if (await check()) return;
    if (processes[0]?.exitCode !== null && processes[0]?.exitCode !== undefined) throw new Error("desktop exited before " + label);
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error("timeout: " + label);
}
function pass(label: string) {
  checks.push(label);
  console.log("PASS: " + label);
}
try {
  const relayPort = await port(),
    workerPort = await port();
  await exec("docker", ["run", "--rm", "-d", "--platform", "linux/amd64", "--name", container, "-p", `127.0.0.1:${relayPort}:8080`, "zork-relay-account-lifecycle:local"]);
  h = await harness({ relay: `http://127.0.0.1:${relayPort}`, origin: `http://127.0.0.1:${workerPort}`, port: workerPort });
  const services = path.join(root, "services.json");
  await fs.writeFile(services, JSON.stringify({ relay_urls: [h.origin], discovery_url: h.origin + "/pkarr" }));
  const env = { ...process.env, ZORK_CLIENT_DATA: root, ZORK_SERVICES_CONFIG: services, ZORK_GUI_PREFERENCES_PATH: path.join(root, "preferences.json"), ZORK_GUI_LOCALE: "zh-CN", ZORK_ACCOUNT_UI_OUTPUT: ui, ZORK_MESH_LOCAL_DISCOVERY: "0", ZORK_MESH_LAN_DISCOVERY: "0", RUST_LOG: "error" };
  const log = await fs.open(path.join(output, "desktop.log"), "w", 0o600);
  const desktop = spawn(path.resolve(testBinary), [], { env, stdio: ["ignore", log.fd, log.fd] });
  processes.push(desktop);
  await until(() => exists(path.join(ui, "authorization.json")), "desktop login intent");
  const authorization = JSON.parse(await fs.readFile(path.join(ui, "authorization.json"), "utf8"));
  const url = new URL(authorization.url);
  assert.equal(url.origin, h.origin);
  const page = await h.fetch(url.pathname);
  const cookie = page.headers.get("set-cookie")!.split(";")[0];
  const csrf = /name="csrf" value="([^"]+)"/.exec(await page.text())![1];
  const consent = await h.fetch(url.pathname, { method: "POST", redirect: "manual", headers: { origin: h.origin, cookie, "content-type": "application/x-www-form-urlencoded" }, body: new URLSearchParams({ csrf }).toString() });
  assert.equal(consent.status, 302);
  const google = new URL(consent.headers.get("location")!);
  const code = ulid();
  h.codes.set(code, { nonce: google.searchParams.get("nonce")!, challenge: google.searchParams.get("code_challenge")!, sub: "desktop-google-user" });
  assert.equal((await h.fetch("/v1/auth/google/callback?" + new URLSearchParams({ state: google.searchParams.get("state")!, code }), { redirect: "manual", headers: { cookie } })).status, 302);
  await until(() => exists(path.join(ui, "signed-in")), "desktop account state");
  pass("real welcome/settings buttons complete Google device login through production core");
  let account = JSON.parse(await fs.readFile(path.join(root, "account/relay.json"), "utf8")).current;
  assert.equal((await fs.stat(path.join(root, "account/relay.json"))).mode & 0o777, 0o600);
  const nodeRoot = path.join(root, "node");
  await exec(path.resolve(binDir, "zork"), ["account", "status", "--data", nodeRoot, "--json"], { env, timeout: 20000 });
  const configPath = path.join(nodeRoot, "config.json");
  const config = JSON.parse(await fs.readFile(configPath, "utf8"));
  for (const key of ["station", "runtime", "control", "agent"]) config.bind[key] = `127.0.0.1:${await port()}`;
  config.mesh = { ...config.mesh, enabled: true, offline: false, bind: "127.0.0.1:0", relay_urls: [h.origin], discovery_url: h.origin + "/pkarr", peers: [], workspaces: [] };
  await fs.writeFile(configPath, JSON.stringify(config));
  const nodeLog = await fs.open(path.join(output, "station.log"), "w", 0o600);
  const station = spawn(path.resolve(binDir, "zork-station"), ["--data", nodeRoot], { env, stdio: ["ignore", nodeLog.fd, nodeLog.fd] });
  processes.push(station);
  await until(async () => {
    const response = await h!.fetch("/v1/auth/session", { headers: { authorization: "Bearer " + account.token } });
    return response.status === 200 && Number(((await response.json()) as any).relay_connections) >= 2;
  }, "owned Station data/enrollment relay connections");
  pass("same-profile Station uses the desktop session for real official-relay handshakes");
  const oldAccess = account.token;
  await exec(path.resolve(binDir, "zork"), ["account", "refresh", "--data", nodeRoot, "--json"], { env, timeout: 20000 });
  account = JSON.parse(await fs.readFile(path.join(root, "account/relay.json"), "utf8")).current;
  assert.notEqual(account.token, oldAccess);
  const accounts: any = await h.mf.getDurableObjectNamespace("ACCOUNTS");
  const owner = accounts.get(accounts.idFromName(account.subject));
  await until(async () => {
    const response = await h!.fetch("/v1/auth/session", { headers: { authorization: "Bearer " + account.token } });
    return Number((await owner.statistics()).quota?.connects) >= 4 && response.status === 200 && Number(((await response.json()) as any).relay_connections) >= 2;
  }, "shared-profile refresh hot reconnect");
  pass("a CLI refresh rotates the shared session while the desktop and Station remain running");
  await fs.writeFile(path.join(ui, "relay-ready"), "ready");
  await until(() => exists(path.join(ui, "complete.json")), "desktop logout");
  assert.equal((await h.fetch("/v1/auth/session", { headers: { authorization: "Bearer " + account.token } })).status, 401);
  assert.equal(station.exitCode, null);
  pass("desktop logout revokes both live Station connections without stopping LAN node");
  await until(async () => desktop.exitCode === 0, "desktop exit");
  for (const image of ["signed-in.png", "signed-out.png"]) await fs.copyFile(path.join(ui, image), path.join(output, image));
  await fs.writeFile(path.join(output, "result.json"), JSON.stringify({ passed: true, checks, fixture: root }, null, 2));
} catch (error) {
  await fs.writeFile(path.join(output, "result.json"), JSON.stringify({ passed: false, checks, fixture: root, error: error instanceof Error ? error.message : "test failed" }, null, 2));
  throw error;
} finally {
  for (const child of processes) if (child.exitCode === null) child.kill("SIGTERM");
  await h?.close();
  await exec("docker", ["rm", "-f", container]).catch(() => {});
}
